//! Wayland clipboard reader & writer using `wl-paste` / `wl-copy`.

use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use workspace_core::clipboard_model::{
    ClipboardMessage, ImageFormat, ValidatedPayload, validate_image_bytes,
};
use workspace_core::error::ErrorCode;

const CMD_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard is empty")]
    Empty,
    #[error("clipboard item marked sensitive/secret and excluded")]
    Excluded,
    #[error("clipboard binary missing (`wl-clipboard`): {0}")]
    MissingTool(String),
    #[error("clipboard command failed: {0}")]
    CommandFailed(String),
    #[error("payload validation error: {0}")]
    Core(#[from] workspace_core::error::CoreError),
}

impl ClipboardError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Empty => ErrorCode::ClipboardEmpty,
            Self::Excluded => ErrorCode::ClipboardExcluded,
            Self::MissingTool(_) | Self::CommandFailed(_) => ErrorCode::ClipboardReadFailed,
            Self::Core(e) => e.code(),
        }
    }
}

/// Read the current Wayland clipboard without logging sensitive payload bytes.
pub async fn read_wayland_clipboard(
    origin: &str,
    sequence: u64,
) -> Result<ClipboardMessage, ClipboardError> {
    let mimes = list_mime_types().await?;
    if mimes.is_empty() {
        return Err(ClipboardError::Empty);
    }

    if mimes
        .iter()
        .any(|m| m.eq_ignore_ascii_case("x-kde-passwordManagerHint"))
        && let Ok(hint) = run_wl_paste_bytes("x-kde-passwordManagerHint").await
        && String::from_utf8_lossy(&hint)
            .trim()
            .eq_ignore_ascii_case("secret")
    {
        return Err(ClipboardError::Excluded);
    }

    // Prefer image MIME only when an actual supported image type is present.
    let preferred_image = ["image/png", "image/jpeg", "image/webp"]
        .into_iter()
        .find(|candidate| mimes.iter().any(|m| m.eq_ignore_ascii_case(candidate)));

    let has_text = mimes.iter().any(|m| {
        let lower = m.to_ascii_lowercase();
        lower == "text/plain;charset=utf-8" || lower == "text/plain" || lower == "utf8_string"
    });

    if let Some(img_mime) = preferred_image {
        // Guard against image-like selection falling back to text when text exists;
        // when no text target exists, propagate the real read/validation error.
        match run_wl_paste_bytes(img_mime).await {
            Ok(bytes) if !bytes.is_empty() => match validate_image_bytes(img_mime, &bytes) {
                Ok(format) => {
                    return Ok(ClipboardMessage::new_image(
                        origin,
                        sequence,
                        format.as_mime(),
                        &bytes,
                    )?);
                }
                Err(err) if !has_text => return Err(err.into()),
                Err(_) => {}
            },
            Ok(_) if !has_text => return Err(ClipboardError::Empty),
            Ok(_) => {}
            Err(err) if !has_text => return Err(err),
            Err(_) => {}
        }
    }

    if has_text {
        let mime = if mimes
            .iter()
            .any(|m| m.eq_ignore_ascii_case("text/plain;charset=utf-8"))
        {
            "text/plain;charset=utf-8"
        } else if mimes.iter().any(|m| m.eq_ignore_ascii_case("text/plain")) {
            "text/plain"
        } else {
            "UTF8_STRING"
        };
        let bytes = run_wl_paste_bytes(mime).await?;
        let text = String::from_utf8(bytes)
            .map_err(|_| ClipboardError::CommandFailed("clipboard text is not valid UTF-8".into()))?;
        if text.is_empty() {
            return Err(ClipboardError::Empty);
        }
        return Ok(ClipboardMessage::new_text(origin, sequence, text)?);
    }

    Err(ClipboardError::Empty)
}

/// Write a validated payload directly to the Wayland clipboard via `wl-copy`.
pub async fn write_wayland_clipboard(payload: &ValidatedPayload) -> Result<(), ClipboardError> {
    let (mime, bytes): (&str, &[u8]) = match payload {
        ValidatedPayload::Text { text, .. } => ("text/plain;charset=utf-8", text.as_bytes()),
        ValidatedPayload::Image { format, bytes, .. } => {
            let m = match format {
                ImageFormat::Png => "image/png",
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::Webp => "image/webp",
            };
            (m, bytes.as_slice())
        }
    };

    let mut child = Command::new("wl-copy")
        .arg("--type")
        .arg(mime)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ClipboardError::MissingTool(format!("wl-copy spawn failed: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bytes)
            .await
            .map_err(|e| ClipboardError::CommandFailed(format!("wl-copy stdin failed: {e}")))?;
        drop(stdin);
    }

    // `wl-copy` forks a background Wayland selection holder that inherits stdio FDs;
    // leaving stderr nulled lets the parent exit immediately without `wait_with_output()` blocking.
    let status = tokio::time::timeout(CMD_TIMEOUT, child.wait())
        .await
        .map_err(|_| ClipboardError::CommandFailed("wl-copy timed out".into()))?
        .map_err(|e| ClipboardError::CommandFailed(format!("wl-copy wait failed: {e}")))?;

    if !status.success() {
        return Err(ClipboardError::CommandFailed(format!(
            "wl-copy exited {status}"
        )));
    }
    Ok(())
}

async fn list_mime_types() -> Result<Vec<String>, ClipboardError> {
    let child = Command::new("wl-paste")
        .arg("--list-types")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ClipboardError::MissingTool(format!("wl-paste spawn failed: {e}")))?;

    let output = tokio::time::timeout(CMD_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ClipboardError::CommandFailed("wl-paste --list-types timed out".into()))?
        .map_err(|e| ClipboardError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.to_ascii_lowercase().contains("nothing is copied")
            || stderr.to_ascii_lowercase().contains("no selection")
        {
            return Err(ClipboardError::Empty);
        }
        return Err(ClipboardError::CommandFailed(stderr.trim().to_string()));
    }

    let types = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    Ok(types)
}

async fn run_wl_paste_bytes(mime: &str) -> Result<Vec<u8>, ClipboardError> {
    let child = Command::new("wl-paste")
        .arg("--no-newline")
        .arg("--type")
        .arg(mime)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ClipboardError::MissingTool(format!("wl-paste spawn failed: {e}")))?;

    let output = tokio::time::timeout(CMD_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ClipboardError::CommandFailed("wl-paste read timed out".into()))?
        .map_err(|e| ClipboardError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(ClipboardError::Empty);
    }
    Ok(output.stdout)
}
