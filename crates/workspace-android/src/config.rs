//! Configuration and Android/host path resolution for `workspaced`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use workspace_core::limits::{
    DEFAULT_ANDROID_LOCAL_PORT, DEFAULT_DISCOVERY_PORT, DEFAULT_TLS_PORT, RECEIVED_SUBDIR_NAME,
};
use workspace_core::store::{StoreError, write_atomic_secure};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidConfig {
    pub device_name: String,
    #[serde(default)]
    pub device_name_customized: bool,
    #[serde(default = "default_android_user_name")]
    pub user_name: String,
    #[serde(default)]
    pub user_name_customized: bool,
    pub tls_port: u16,
    pub discovery_port: u16,
    pub local_ipc_port: u16,
    pub receive_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub bridge_jar: PathBuf,
    pub sync_paused: bool,
    pub gesture_enabled: bool,
    pub gesture_fingers: u8,
    pub swipe_up_action: String,
    pub swipe_down_action: String,
}

impl Default for AndroidConfig {
    fn default() -> Self {
        Self {
            device_name: detect_android_device_name(),
            device_name_customized: false,
            user_name: default_android_user_name(),
            user_name_customized: false,
            tls_port: DEFAULT_TLS_PORT,
            discovery_port: DEFAULT_DISCOVERY_PORT,
            local_ipc_port: DEFAULT_ANDROID_LOCAL_PORT,
            receive_dir: default_receive_dir(),
            staging_dir: PathBuf::from("/data/local/tmp/workspace"),
            bridge_jar: default_bridge_jar_path(),
            sync_paused: false,
            gesture_enabled: true,
            gesture_fingers: 2,
            swipe_up_action: "push".to_string(),
            swipe_down_action: "pull".to_string(),
        }
    }
}

pub fn default_android_user_name() -> String {
    "User".to_string()
}

pub fn default_state_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/data/adb/workspace/state")
    } else {
        PathBuf::from("/tmp/workspace-android-test/state")
    }
}

pub fn default_receive_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/storage/emulated/0/Download").join(RECEIVED_SUBDIR_NAME)
    } else {
        PathBuf::from("/tmp/workspace-android-test/Download").join(RECEIVED_SUBDIR_NAME)
    }
}

pub fn default_bridge_jar_path() -> PathBuf {
    // Must stay inside `/data/local/tmp/workspace` (0775 system:shell) because UID 1000/2000 cannot traverse `/data/adb` (0700).
    PathBuf::from("/data/local/tmp/workspace/workspace-bridge.jar")
}

pub(crate) fn clean_android_value(raw: &str) -> Option<String> {
    let trimmed = raw.replace(['\r', '\n'], " ").trim().to_string();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(trimmed.chars().take(64).collect())
    }
}

pub fn detect_android_device_name() -> String {
    if cfg!(target_os = "android") {
        for args in [
            ["get", "global", "device_name"],
            ["get", "secure", "bluetooth_name"],
        ] {
            if let Ok(out) = std::process::Command::new("/system/bin/settings")
                .args(args)
                .output()
                && out.status.success()
            {
                let raw = String::from_utf8_lossy(&out.stdout);
                if let Some(cleaned) = clean_android_value(&raw) {
                    return cleaned;
                }
            }
        }
        for prop in [
            "persist.sys.device_name",
            "persist.sys.oplus.device_name",
            "ro.product.marketname",
            "ro.vendor.oplus.market.name",
            "ro.product.model",
        ] {
            if let Ok(out) = std::process::Command::new("getprop").arg(prop).output()
                && out.status.success()
            {
                let raw = String::from_utf8_lossy(&out.stdout);
                if let Some(cleaned) = clean_android_value(&raw) {
                    return cleaned;
                }
            }
        }
        return "OPPO Android".to_string();
    }
    "AndroidDevHost".to_string()
}

pub fn load_or_create_config(state_dir: &Path) -> Result<AndroidConfig, StoreError> {
    let path = state_dir.join("config.toml");
    if path.exists() {
        let raw = fs::read_to_string(&path).map_err(|e| StoreError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let mut cfg: AndroidConfig = toml::from_str(&raw).map_err(|e| StoreError::TomlDe {
            path: path.display().to_string(),
            source: e,
        })?;
        let mut dirty = false;
        if !cfg.device_name_customized || cfg.device_name.trim().is_empty() {
            let detected = detect_android_device_name();
            if cfg.device_name != detected {
                cfg.device_name = detected;
                dirty = true;
            }
        }
        if !cfg.user_name_customized || cfg.user_name.trim().is_empty() {
            let default_user = default_android_user_name();
            if cfg.user_name != default_user {
                cfg.user_name = default_user;
                dirty = true;
            }
        }
        if cfg.bridge_jar.starts_with("/data/adb/") {
            cfg.bridge_jar = default_bridge_jar_path();
            dirty = true;
        }
        if dirty || !raw.contains("user_name") {
            let _ = save_config(state_dir, &cfg);
        }
        return Ok(cfg);
    }

    let cfg = AndroidConfig::default();
    save_config(state_dir, &cfg)?;
    Ok(cfg)
}

pub fn save_config(state_dir: &Path, cfg: &AndroidConfig) -> Result<(), StoreError> {
    let serialized = toml::to_string_pretty(cfg)?;
    write_atomic_secure(&state_dir.join("config.toml"), serialized.as_bytes(), 0o600)
}
