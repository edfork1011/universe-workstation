//! Privilege-dropped (`UID 2000` shell) `app_process` bridge to `ClipboardBridge.java`,
//! plus MediaStore scan & notification helpers.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use tokio::process::Command;
use tracing::{debug, warn};
use uuid::Uuid;
use workspace_core::clipboard_model::{
    ClipboardMessage, ImageFormat, ValidatedPayload, detect_and_validate_image,
};
use workspace_core::limits::MAX_TEXT_BYTES;

const BRIDGE_CLASS: &str = "me.edfork.workspace.bridge.ClipboardBridge";
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct BridgeJsonOut {
    ok: bool,
    kind: Option<String>,
    data_b64: Option<String>,
    file: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

#[derive(Clone)]
pub struct AppProcessBridge {
    bridge_jar: PathBuf,
    staging_dir: PathBuf,
}

impl AppProcessBridge {
    pub fn new(bridge_jar: PathBuf, staging_dir: PathBuf) -> Self {
        Self {
            bridge_jar,
            staging_dir,
        }
    }

    pub fn ensure_staging_dir(&self) {
        if let Err(err) = std::fs::create_dir_all(&self.staging_dir) {
            warn!("failed to create bridge staging dir: {err}");
            return;
        }
        // `0775` system:shell ensures UID 1000 `app_process` and UID 2000 shell can traverse & stage files.
        let _ = std::fs::set_permissions(&self.staging_dir, std::fs::Permissions::from_mode(0o775));
        unsafe {
            if libc::geteuid() == 0 {
                let c_dir = std::ffi::CString::new(self.staging_dir.to_string_lossy().as_bytes())
                    .unwrap_or_default();
                let _ = libc::chown(c_dir.as_ptr(), 1000, 2000);
                self.sync_bridge_jar();
            }
        }
    }

    fn sync_bridge_jar(&self) {
        for src in [
            "/data/adb/workspace/dex/workspace-bridge.jar",
            "/data/adb/modules/me_edfork_workspace/dex/workspace-bridge.jar",
        ] {
            let src_path = Path::new(src);
            if !src_path.is_file() {
                continue;
            }
            let needs_copy = match (
                std::fs::metadata(src_path),
                std::fs::metadata(&self.bridge_jar),
            ) {
                (Ok(src_meta), Ok(dst_meta)) => {
                    let src_mtime = src_meta.modified().ok();
                    let dst_mtime = dst_meta.modified().ok();
                    src_meta.len() != dst_meta.len()
                        || match (src_mtime, dst_mtime) {
                            (Some(s), Some(d)) => s > d,
                            _ => false,
                        }
                }
                _ => true,
            };
            if needs_copy && std::fs::copy(src_path, &self.bridge_jar).is_ok() {
                let _ = std::fs::set_permissions(
                    &self.bridge_jar,
                    std::fs::Permissions::from_mode(0o644),
                );
                if let Ok(c_jar) =
                    std::ffi::CString::new(self.bridge_jar.to_string_lossy().as_bytes())
                {
                    unsafe {
                        let _ = libc::chown(c_jar.as_ptr(), 1000, 2000);
                    }
                }
            }
            break;
        }
    }

    /// Ensure system (`android`), `com.android.shell`, and `me.edfork.workspace` hold clipboard appops.
    pub async fn ensure_shell_clipboard_appops(&self) {
        for pkg in ["android", "com.android.shell", "me.edfork.workspace"] {
            for op in ["READ_CLIPBOARD", "WRITE_CLIPBOARD"] {
                let _ = Command::new("/system/bin/cmd")
                    .args(["appops", "set", pkg, op, "allow"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }
    }

    async fn run_bridge(&self, args: &[&str]) -> Result<String, String> {
        self.ensure_staging_dir();
        if !self.bridge_jar.is_file() {
            return Err(format!(
                "workspace-bridge.jar 不存在: {}",
                self.bridge_jar.display()
            ));
        }
        let classpath = self.bridge_jar.display().to_string();

        let mut cmd = Command::new("/system/bin/app_process");
        cmd.arg("/system/bin")
            .arg(BRIDGE_CLASS)
            .args(args)
            .env("CLASSPATH", classpath)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Drop root -> UID/GID 1000 (AID_SYSTEM) with supplementary groups so IClipboard
        // accepts package="android" (holds READ_CLIPBOARD_IN_BACKGROUND) and can call clipprovider.
        unsafe {
            cmd.pre_exec(|| {
                if libc::geteuid() == 0 {
                    let groups: [libc::gid_t; 5] = [1000, 2000, 1015, 1028, 3003];
                    if libc::setgroups(groups.len() as _, groups.as_ptr()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::setgid(1000) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::setuid(1000) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn app_process: {e}"))?;
        let output = tokio::time::timeout(BRIDGE_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| "ClipboardBridge app_process timed out".to_string())?
            .map_err(|e| format!("ClipboardBridge wait failed: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!(
                "ClipboardBridge returned empty output (status={}, stderr={})",
                output.status, stderr
            ));
        }
        Ok(stdout)
    }

    pub async fn read_clipboard(&self, origin: &str, sequence: u64) -> Result<ClipboardMessage, String> {
        let staging_str = self.staging_dir.display().to_string();
        let line = self.run_bridge(&["read", &staging_str]).await?;
        let parsed: BridgeJsonOut = serde_json::from_str(
            line.lines()
                .last()
                .unwrap_or(&line),
        )
        .map_err(|e| format!("invalid bridge JSON `{line}`: {e}"))?;

        if !parsed.ok {
            let code = parsed.code.unwrap_or_else(|| "CLIPBOARD_EMPTY".to_string());
            let msg = parsed.message.unwrap_or_else(|| "clipboard read failed".to_string());
            return Err(format!("{code}: {msg}"));
        }

        match parsed.kind.as_deref() {
            Some("text") => {
                let b64 = parsed
                    .data_b64
                    .ok_or_else(|| "bridge missing data_b64".to_string())?;
                let bytes = STANDARD
                    .decode(b64.trim())
                    .map_err(|e| format!("invalid text base64 from bridge: {e}"))?;
                if bytes.len() > MAX_TEXT_BYTES {
                    return Err("TEXT_TOO_LARGE: clipboard text exceeds 1 MiB".to_string());
                }
                let text = String::from_utf8(bytes)
                    .map_err(|e| format!("clipboard text not UTF-8: {e}"))?;
                ClipboardMessage::new_text(origin, sequence, text).map_err(|e| e.to_string())
            }
            Some("image") => {
                let path = PathBuf::from(
                    parsed
                        .file
                        .ok_or_else(|| "bridge missing staged image path".to_string())?,
                );
                let raw = tokio::fs::read(&path)
                    .await
                    .map_err(|e| format!("failed to read staged clipboard image: {e}"))?;
                let _ = tokio::fs::remove_file(&path).await;
                let format = detect_and_validate_image(&raw).map_err(|e| e.to_string())?;
                ClipboardMessage::new_image(origin, sequence, format.as_mime(), &raw)
                    .map_err(|e| e.to_string())
            }
            other => Err(format!("unexpected bridge response kind: {other:?}")),
        }
    }

    pub async fn write_clipboard(&self, payload: &ValidatedPayload) -> Result<(), String> {
        match payload {
            ValidatedPayload::Text { text, .. } => {
                let b64 = STANDARD.encode(text.as_bytes());
                let line = self.run_bridge(&["write-text", &b64]).await?;
                let parsed: BridgeJsonOut = serde_json::from_str(line.lines().last().unwrap_or(&line))
                    .map_err(|e| format!("invalid bridge response: {e}"))?;
                if parsed.ok {
                    Ok(())
                } else {
                    Err(parsed.message.unwrap_or_else(|| "write-text failed".to_string()))
                }
            }
            ValidatedPayload::Image { format, bytes, .. } => {
                self.ensure_staging_dir();
                let ext = match format {
                    ImageFormat::Png => "png",
                    ImageFormat::Jpeg => "jpg",
                    ImageFormat::Webp => "webp",
                };
                let staged = self
                    .staging_dir
                    .join(format!("clip_write_{}.{ext}", Uuid::new_v4()));
                tokio::fs::write(&staged, bytes)
                    .await
                    .map_err(|e| format!("failed to write staged image: {e}"))?;
                let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o644));

                let staged_str = staged.display().to_string();
                let mime = format.as_mime();
                let line_res = self
                    .run_bridge(&["write-image", mime, &staged_str])
                    .await;
                let _ = tokio::fs::remove_file(&staged).await;

                let line = line_res?;
                let parsed: BridgeJsonOut = serde_json::from_str(line.lines().last().unwrap_or(&line))
                    .map_err(|e| format!("invalid bridge response: {e}"))?;
                if parsed.ok {
                    Ok(())
                } else {
                    Err(parsed.message.unwrap_or_else(|| "write-image failed".to_string()))
                }
            }
        }
    }

    pub async fn scan_media(&self, file_path: &Path) {
        let path_str = file_path.display().to_string();
        if let Err(err) = self.run_bridge(&["scan-media", &path_str]).await {
            debug!("scan-media bridge skipped ({err})");
        }
    }

    pub fn notify_system(&self, title: &str, body: &str) {
        let title = title.to_string();
        let body = body.to_string();
        tokio::spawn(async move {
            let tag = format!("ws_{}", Uuid::new_v4());
            let _ = Command::new("/system/bin/cmd")
                .args([
                    "notification",
                    "post",
                    "-S",
                    "bigtext",
                    "-t",
                    &title,
                    &tag,
                    &body,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        });
    }
}
