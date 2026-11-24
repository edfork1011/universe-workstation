//! Atomic `0600` persistence for device identity (`identity.toml`) and trusted peers (`trusted_peers.toml`).

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::crypto::{CryptoError, DeviceIdentity, TrustPredicate, encode_pubkey_b64};
use crate::protocol::{Platform, TrustedPeer};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML decode error at {path}: {source}")]
    TomlDe {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("TOML encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("cryptographic identity error: {0}")]
    Crypto(#[from] CryptoError),
    #[error("base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
}

fn io_err(path: &Path, source: std::io::Error) -> StoreError {
    StoreError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Ensure directory exists with private (`0700`) permissions when created.
pub fn ensure_private_dir(dir: &Path) -> Result<(), StoreError> {
    if !dir.exists() {
        fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    }
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Write `bytes` atomically via a temporary sibling file, `0600` permissions, `sync_all`, and rename.
pub fn write_atomic_secure(path: &Path, bytes: &[u8], mode: u32) -> Result<(), StoreError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_private_dir(parent)?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("state.toml");
    let mut suffix = [0u8; 8];
    OsRng.fill_bytes(&mut suffix);
    let tmp_name = format!(
        ".{file_name}.tmp.{}.{:02x}{:02x}{:02x}{:02x}",
        std::process::id(),
        suffix[0],
        suffix[1],
        suffix[2],
        suffix[3]
    );
    let tmp_path = parent.join(tmp_name);

    {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(&tmp_path)
            .map_err(|e| io_err(&tmp_path, e))?;
        file.write_all(bytes).map_err(|e| io_err(&tmp_path, e))?;
        file.sync_all().map_err(|e| io_err(&tmp_path, e))?;
    }
    #[cfg(unix)]
    {
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode))
            .map_err(|e| io_err(&tmp_path, e))?;
    }

    fs::rename(&tmp_path, path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        io_err(path, e)
    })?;

    #[cfg(unix)]
    {
        if let Ok(dir_file) = fs::File::open(parent) {
            let _ = dir_file.sync_all();
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdentityFile {
    device_name: String,
    platform: Platform,
    pkcs8_b64: String,
    cert_b64: String,
}

/// Load existing Ed25519 + X.509 identity from `state_dir/identity.toml`, or generate and save one.
pub fn load_or_create_identity(
    state_dir: &Path,
    default_device_name: &str,
    platform: Platform,
) -> Result<DeviceIdentity, StoreError> {
    let path = state_dir.join("identity.toml");
    if path.exists() {
        let raw = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
        let parsed: IdentityFile = toml::from_str(&raw).map_err(|e| StoreError::TomlDe {
            path: path.display().to_string(),
            source: e,
        })?;
        let pkcs8 = STANDARD.decode(parsed.pkcs8_b64.trim())?;
        let cert = STANDARD.decode(parsed.cert_b64.trim())?;
        let name = if parsed.device_name.trim().is_empty() {
            default_device_name.to_string()
        } else {
            parsed.device_name
        };
        return Ok(DeviceIdentity::from_stored(name, parsed.platform, &pkcs8, &cert)?);
    }

    let identity = DeviceIdentity::generate(default_device_name, platform)?;
    save_identity(state_dir, &identity)?;
    Ok(identity)
}

/// Persist `identity` atomically to `state_dir/identity.toml` (`0600`).
pub fn save_identity(state_dir: &Path, identity: &DeviceIdentity) -> Result<(), StoreError> {
    let file = IdentityFile {
        device_name: identity.device_name(),
        platform: identity.platform,
        pkcs8_b64: STANDARD.encode(identity.pkcs8_der()),
        cert_b64: STANDARD.encode(identity.cert_der().as_ref()),
    };
    let serialized = toml::to_string_pretty(&file)?;
    write_atomic_secure(&state_dir.join("identity.toml"), serialized.as_bytes(), 0o600)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustedPeersFile {
    #[serde(default)]
    peers: Vec<TrustedPeer>,
}

/// Thread-safe persistent registry of authenticated & paired peers.
#[derive(Debug, Clone)]
pub struct TrustRegistry {
    path: PathBuf,
    inner: Arc<RwLock<TrustedPeersFile>>,
}

impl TrustRegistry {
    /// Load or initialize `state_dir/trusted_peers.toml`.
    pub fn load_in_dir(state_dir: &Path) -> Result<Self, StoreError> {
        Self::load_or_init(&state_dir.join("trusted_peers.toml"))
    }

    /// Load or initialize trusted peers store at `path`.
    pub fn load_or_init(path: &Path) -> Result<Self, StoreError> {
        let data = if path.exists() {
            let raw = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
            toml::from_str::<TrustedPeersFile>(&raw).map_err(|e| StoreError::TomlDe {
                path: path.display().to_string(),
                source: e,
            })?
        } else {
            TrustedPeersFile::default()
        };
        Ok(Self {
            path: path.to_path_buf(),
            inner: Arc::new(RwLock::new(data)),
        })
    }

    fn persist_locked(&self, guard: &TrustedPeersFile) -> Result<(), StoreError> {
        let serialized = toml::to_string_pretty(guard)?;
        write_atomic_secure(&self.path, serialized.as_bytes(), 0o600)
    }

    /// Check whether raw 32-byte Ed25519 public key currently belongs to a paired peer.
    pub fn is_pubkey_trusted(&self, pubkey: &[u8; 32]) -> bool {
        let target_b64 = encode_pubkey_b64(pubkey);
        let guard = self.inner.read().expect("TrustRegistry lock poisoned");
        guard.peers.iter().any(|p| p.pubkey_b64 == target_b64)
    }

    /// Look up a trusted peer by `device_id`.
    pub fn get_by_device_id(&self, device_id: &str) -> Option<TrustedPeer> {
        let guard = self.inner.read().expect("TrustRegistry lock poisoned");
        guard
            .peers
            .iter()
            .find(|p| p.device_id == device_id)
            .cloned()
    }

    /// Look up a trusted peer by raw 32-byte Ed25519 public key.
    pub fn get_by_pubkey(&self, pubkey: &[u8; 32]) -> Option<TrustedPeer> {
        let target_b64 = encode_pubkey_b64(pubkey);
        let guard = self.inner.read().expect("TrustRegistry lock poisoned");
        guard
            .peers
            .iter()
            .find(|p| p.pubkey_b64 == target_b64)
            .cloned()
    }

    /// List all trusted peers sorted by paired timestamp / device name.
    pub fn list(&self) -> Vec<TrustedPeer> {
        let guard = self.inner.read().expect("TrustRegistry lock poisoned");
        guard.peers.clone()
    }

    /// Add or update a trusted peer and persist atomically to disk.
    pub fn upsert(&self, peer: TrustedPeer) -> Result<(), StoreError> {
        let mut guard = self.inner.write().expect("TrustRegistry lock poisoned");
        if let Some(existing) = guard
            .peers
            .iter_mut()
            .find(|p| p.device_id == peer.device_id || p.pubkey_b64 == peer.pubkey_b64)
        {
            *existing = peer;
        } else {
            guard.peers.push(peer);
        }
        self.persist_locked(&guard)
    }

    /// Update `last_known_ip`, `tls_port`, and optionally `device_name` only when changed.
    pub fn update_seen_endpoint(
        &self,
        device_id: &str,
        ip: Option<&str>,
        tls_port: u16,
        device_name: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut guard = self.inner.write().expect("TrustRegistry lock poisoned");
        let Some(existing) = guard.peers.iter_mut().find(|p| p.device_id == device_id) else {
            return Ok(());
        };
        let mut changed = false;
        if let Some(new_ip) = ip
            && existing.last_known_ip.as_deref() != Some(new_ip)
        {
            existing.last_known_ip = Some(new_ip.to_string());
            changed = true;
        }
        if tls_port != 0 && existing.tls_port != tls_port {
            existing.tls_port = tls_port;
            changed = true;
        }
        if let Some(name) = device_name
            && !name.trim().is_empty()
            && existing.device_name != name
        {
            existing.device_name = name.to_string();
            changed = true;
        }
        if changed {
            self.persist_locked(&guard)?;
        }
        Ok(())
    }

    /// Revoke/unpair a peer immediately from memory and persistent store.
    pub fn remove(&self, device_id: &str) -> Result<Option<TrustedPeer>, StoreError> {
        let mut guard = self.inner.write().expect("TrustRegistry lock poisoned");
        let Some(idx) = guard.peers.iter().position(|p| p.device_id == device_id) else {
            return Ok(None);
        };
        let removed = guard.peers.remove(idx);
        self.persist_locked(&guard)?;
        Ok(Some(removed))
    }

    /// Dynamic predicate suitable for `build_mtls_server_config_with_filter`.
    pub fn trust_predicate(&self) -> TrustPredicate {
        let cloned = self.clone();
        Arc::new(move |pk| cloned.is_pubkey_trusted(pk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn identity_persists_and_reloads_stably() {
        let dir = TempDir::new().unwrap();
        let id1 = load_or_create_identity(dir.path(), "Workstation", Platform::Linux).unwrap();
        let id2 = load_or_create_identity(dir.path(), "Ignored", Platform::Linux).unwrap();
        assert_eq!(id1.device_id, id2.device_id);
        assert_eq!(id1.pubkey_bytes(), id2.pubkey_bytes());
        assert_eq!(id1.cert_sha256_hex(), id2.cert_sha256_hex());
    }

    #[test]
    fn trust_registry_upsert_lookup_and_revoke() {
        let dir = TempDir::new().unwrap();
        let reg = TrustRegistry::load_in_dir(dir.path()).unwrap();
        let id = DeviceIdentity::generate("Phone", Platform::Android).unwrap();
        let pk = id.pubkey_bytes();
        assert!(!reg.is_pubkey_trusted(&pk));

        reg.upsert(TrustedPeer {
            device_id: id.device_id.clone(),
            device_name: "OPPO PLP110".into(),
            platform: Platform::Android,
            pubkey_b64: id.pubkey_b64(),
            cert_sha256_hex: id.cert_sha256_hex(),
            paired_at_ms: 12345,
            last_known_ip: Some("192.168.1.50".into()),
            tls_port: 45877,
        })
        .unwrap();

        let reloaded = TrustRegistry::load_in_dir(dir.path()).unwrap();
        assert!(reloaded.is_pubkey_trusted(&pk));
        assert_eq!(reloaded.list().len(), 1);

        let pred = reg.trust_predicate();
        assert!(pred(&pk));

        let removed = reg.remove(&id.device_id).unwrap();
        assert!(removed.is_some());
        assert!(!pred(&pk));
    }
}
