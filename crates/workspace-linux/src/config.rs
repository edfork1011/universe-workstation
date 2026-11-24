//! Configuration and XDG path resolution for `me-edfork-workspace`.

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use workspace_core::limits::{DEFAULT_DISCOVERY_PORT, DEFAULT_TLS_PORT, RECEIVED_SUBDIR_NAME};
use workspace_core::store::{StoreError, write_atomic_secure};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinuxConfig {
    pub device_name: String,
    #[serde(default)]
    pub device_name_customized: bool,
    #[serde(default = "default_linux_user_name")]
    pub user_name: String,
    #[serde(default)]
    pub user_name_customized: bool,
    pub tls_port: u16,
    pub discovery_port: u16,
    pub sync_paused: bool,
    pub receive_dir: PathBuf,
}

impl Default for LinuxConfig {
    fn default() -> Self {
        Self {
            device_name: detect_linux_device_name(),
            device_name_customized: false,
            user_name: default_linux_user_name(),
            user_name_customized: false,
            tls_port: DEFAULT_TLS_PORT,
            discovery_port: DEFAULT_DISCOVERY_PORT,
            sync_paused: false,
            receive_dir: default_receive_dir(),
        }
    }
}

/// Detect the local Linux host display name (`PRETTY_HOSTNAME` in `/etc/machine-info` or system hostname).
pub fn detect_linux_device_name() -> String {
    if let Ok(raw) = fs::read_to_string("/etc/machine-info") {
        for line in raw.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("PRETTY_HOSTNAME=") {
                let cleaned = rest.trim().trim_matches('"').trim_matches('\'').trim();
                if !cleaned.is_empty() {
                    return cleaned.to_string();
                }
            }
        }
    }
    hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ArchLinux".to_string())
}

/// Resolve the default local Linux username preserving original casing (never forced Title-Case).
pub fn default_linux_user_name() -> String {
    for key in ["USER", "LOGNAME"] {
        if let Ok(raw) = std::env::var(key) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            let name = CStr::from_ptr((*pw).pw_name).to_string_lossy().trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "User".to_string()
}

pub fn default_state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/state")
        })
        .join("me.edfork.workspace")
}

pub fn default_receive_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("Downloads")
        })
        .join(RECEIVED_SUBDIR_NAME)
}

pub fn runtime_socket_path() -> PathBuf {
    if let Some(runtime) = dirs::runtime_dir() {
        return runtime.join("me.edfork.workspace.sock");
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/me.edfork.workspace-{uid}.sock"))
}

pub fn load_or_create_config(state_dir: &Path) -> Result<LinuxConfig, StoreError> {
    let path = state_dir.join("config.toml");
    if path.exists() {
        let raw = fs::read_to_string(&path).map_err(|e| StoreError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let mut cfg: LinuxConfig = toml::from_str(&raw).map_err(|e| StoreError::TomlDe {
            path: path.display().to_string(),
            source: e,
        })?;
        let mut dirty = false;
        if !cfg.device_name_customized || cfg.device_name.trim().is_empty() {
            let detected = detect_linux_device_name();
            if cfg.device_name != detected {
                cfg.device_name = detected;
                dirty = true;
            }
        }
        if !cfg.user_name_customized || cfg.user_name.trim().is_empty() {
            let default_user = default_linux_user_name();
            if cfg.user_name != default_user {
                cfg.user_name = default_user;
                dirty = true;
            }
        }
        if dirty || !raw.contains("user_name") {
            let _ = save_config(state_dir, &cfg);
        }
        return Ok(cfg);
    }

    let cfg = LinuxConfig::default();
    save_config(state_dir, &cfg)?;
    Ok(cfg)
}

pub fn save_config(state_dir: &Path, cfg: &LinuxConfig) -> Result<(), StoreError> {
    let serialized = toml::to_string_pretty(cfg)?;
    write_atomic_secure(&state_dir.join("config.toml"), serialized.as_bytes(), 0o600)
}
