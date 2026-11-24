//! Core Linux daemon: background LAN peer monitoring, mTLS server, Wayland clipboard sync,
//! LAN file reception/sending, and local Unix socket IPC (`me.edfork.workspace.sock`).

use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, info, warn};
use uuid::Uuid;
use workspace_core::clipboard_model::{
    ClipboardMessage, SnapshotMetadata, ValidatedPayload, now_timestamp_ms,
};
use workspace_core::crypto::DeviceIdentity;
use workspace_core::discovery::{DiscoveryConfig, DiscoveryHandle};
use workspace_core::error::{ApiErrorBody, ErrorCode};
use workspace_core::limits::{PROTOCOL_VERSION, format_human_size};
use workspace_core::mtls_http::{
    MtlsClient, MtlsServerHandle, PairingManager, PeerCallbacks, TlsAuthPolicy, UploadProgressFn,
    start_mtls_server,
};
use workspace_core::protocol::{
    ClipboardPushResponse, ClipboardSummary, DiscoveredPeer, PairedPeerStatus, PairState,
    PendingPairView, Platform, TransferDirection, TransferRecord, TransferState, TrustedPeer,
};
use workspace_core::store::{TrustRegistry, load_or_create_identity};
use workspace_core::transfer::ReceivedFile;

use crate::clipboard::read_wayland_clipboard;
use crate::config::{
    LinuxConfig, default_linux_user_name, default_state_dir, detect_linux_device_name,
    load_or_create_config, runtime_socket_path, save_config,
};
use crate::dedup::DedupTracker;
use crate::notify::send_notification;
use crate::tray::{TrayAction, TrayStatus, spawn_tray_service};
use crate::watcher::{LatestSnapshot, SharedDedup, apply_remote_clipboard, spawn_clipboard_watcher};

const MAX_TRANSFER_HISTORY: usize = 32;
const MONITOR_INTERVAL: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSnapshot {
    pub protocol: u32,
    pub device_id: String,
    pub device_name: String,
    pub user_name: String,
    pub fingerprint: String,
    pub tls_port: u16,
    pub discovery_port: u16,
    pub sync_paused: bool,
    pub receive_dir: String,
    pub paired_peers: Vec<PairedPeerStatus>,
    pub discovered_peers: Vec<DiscoveredPeer>,
    pub pending_pairs: Vec<PendingPairView>,
    pub last_clipboard: Option<ClipboardSummary>,
    pub transfers: Vec<TransferRecord>,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    Snapshot(Box<DaemonSnapshot>),
    ShowWindow(Option<String>),
    Quit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    ShowWindow {
        #[serde(default)]
        activation_token: Option<String>,
    },
    GetState,
    StartPair {
        device_id: String,
    },
    ApprovePair {
        session_id: String,
    },
    RejectPair {
        session_id: String,
    },
    Unpair {
        device_id: String,
    },
    SetSyncPaused {
        paused: bool,
    },
    SendFiles {
        peer_id: String,
        paths: Vec<PathBuf>,
        #[serde(default)]
        stream: bool,
    },
    CancelTransfer {
        batch_id: String,
    },
    Quit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Ok,
    State { snapshot: Box<DaemonSnapshot> },
    PairSession { session: PendingPairView },
    TransferProgress { record: TransferRecord },
    TransferFinished { record: TransferRecord },
    Error { message: String },
}

/// Complete live runtime state shared cleanly between mTLS callbacks, monitor loop, tray, and UI.
#[derive(Clone)]
pub struct DaemonRuntime {
    pub state_dir: PathBuf,
    pub identity: Arc<DeviceIdentity>,
    pub trust: TrustRegistry,
    pub pairing: PairingManager,
    pub discovery: DiscoveryHandle,
    pub config: Arc<RwLock<LinuxConfig>>,
    pub dedup: SharedDedup,
    pub latest_snapshot: LatestSnapshot,
    peer_status: Arc<RwLock<HashMap<String, PairedPeerStatus>>>,
    last_clipboard: Arc<RwLock<Option<ClipboardSummary>>>,
    transfers: Arc<RwLock<VecDeque<TransferRecord>>>,
    active_cancels: Arc<RwLock<HashMap<String, watch::Sender<bool>>>>,
    ui_tx: broadcast::Sender<UiEvent>,
    tray_tx: watch::Sender<TrayStatus>,
    shutdown_tx: watch::Sender<bool>,
}

impl DaemonRuntime {
    pub fn snapshot(&self) -> DaemonSnapshot {
        let cfg = self.config.read().expect("config lock poisoned").clone();
        let trusted = self.trust.list();
        let status_map = self
            .peer_status
            .read()
            .expect("peer_status lock poisoned")
            .clone();

        let mut paired_peers: Vec<PairedPeerStatus> = trusted
            .iter()
            .map(|tp| {
                status_map.get(&tp.device_id).cloned().unwrap_or_else(|| {
                    PairedPeerStatus {
                        device_id: tp.device_id.clone(),
                        device_name: tp.device_name.clone(),
                        platform: tp.platform,
                        fingerprint: String::new(),
                        connected: false,
                        ip: tp.last_known_ip.clone(),
                        tls_port: tp.tls_port,
                        rtt_ms: None,
                        last_seen_ms: None,
                    }
                })
            })
            .collect();
        paired_peers.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then(a.device_name.cmp(&b.device_name))
        });

        let discovered_peers: Vec<DiscoveredPeer> = self
            .discovery
            .list_peers()
            .into_iter()
            .filter(|p| !p.is_trusted)
            .collect();
        let pending_pairs = self.pairing.list_pending();
        let last_clipboard = self
            .last_clipboard
            .read()
            .expect("last_clipboard lock poisoned")
            .clone();
        let transfers: Vec<TransferRecord> = self
            .transfers
            .read()
            .expect("transfers lock poisoned")
            .iter()
            .cloned()
            .collect();

        DaemonSnapshot {
            protocol: PROTOCOL_VERSION,
            device_id: self.identity.device_id.clone(),
            device_name: self.identity.device_name(),
            user_name: cfg.user_name.clone(),
            fingerprint: self.identity.fingerprint(),
            tls_port: cfg.tls_port,
            discovery_port: cfg.discovery_port,
            sync_paused: cfg.sync_paused,
            receive_dir: cfg.receive_dir.display().to_string(),
            paired_peers,
            discovered_peers,
            pending_pairs,
            last_clipboard,
            transfers,
        }
    }

    pub fn publish_snapshot(&self) {
        let snap = self.snapshot();
        let connected_names: Vec<&str> = snap
            .paired_peers
            .iter()
            .filter(|p| p.connected)
            .map(|p| p.device_name.as_str())
            .collect();
        let connected = !connected_names.is_empty();
        let peer_summary = if connected {
            connected_names.join("、")
        } else if let Some(first) = snap.paired_peers.first() {
            format!("{} (离线)", first.device_name)
        } else {
            "未配对设备".to_string()
        };
        let _ = self.tray_tx.send(TrayStatus {
            connected,
            peer_summary,
            sync_paused: snap.sync_paused,
        });
        let _ = self.ui_tx.send(UiEvent::Snapshot(Box::new(snap)));
    }

    pub fn request_show_window(&self, activation_token: Option<String>) {
        let _ = self.ui_tx.send(UiEvent::ShowWindow(activation_token));
    }

    pub fn request_quit(&self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.ui_tx.send(UiEvent::Quit);
    }

    pub fn subscribe_ui(&self) -> broadcast::Receiver<UiEvent> {
        self.ui_tx.subscribe()
    }

    pub fn set_sync_paused(&self, paused: bool) -> Result<(), String> {
        {
            let mut cfg = self.config.write().expect("config lock poisoned");
            cfg.sync_paused = paused;
            save_config(&self.state_dir, &cfg).map_err(|e| e.to_string())?;
        }
        self.publish_snapshot();
        Ok(())
    }

    pub fn set_user_name(&self, raw: &str) -> Result<(), String> {
        let cleaned: String = raw
            .replace(['\r', '\n'], " ")
            .trim()
            .chars()
            .take(64)
            .collect();
        {
            let mut cfg = self.config.write().expect("config lock poisoned");
            if cleaned.is_empty() {
                cfg.user_name_customized = false;
                cfg.user_name = default_linux_user_name();
            } else {
                cfg.user_name_customized = true;
                cfg.user_name = cleaned;
            }
            save_config(&self.state_dir, &cfg).map_err(|e| e.to_string())?;
        }
        self.publish_snapshot();
        Ok(())
    }

    pub async fn set_device_name(&self, raw: &str) -> Result<(), String> {
        let cleaned: String = raw
            .replace(['\r', '\n'], " ")
            .trim()
            .chars()
            .take(64)
            .collect();
        let final_name = {
            let mut cfg = self.config.write().expect("config lock poisoned");
            if cleaned.is_empty() {
                cfg.device_name_customized = false;
                cfg.device_name = detect_linux_device_name();
            } else {
                cfg.device_name_customized = true;
                cfg.device_name = cleaned;
            }
            save_config(&self.state_dir, &cfg).map_err(|e| e.to_string())?;
            cfg.device_name.clone()
        };
        self.identity.set_device_name(final_name);
        let _ = workspace_core::store::save_identity(&self.state_dir, &self.identity);
        self.discovery.announce_now().await;
        self.publish_snapshot();
        Ok(())
    }

    pub async fn start_pair(&self, device_id: &str) -> Result<PendingPairView, String> {
        let peer = self
            .discovery
            .get_peer(device_id)
            .ok_or_else(|| "未在局域网内找到该设备，请确认两者在同一 Wi-Fi".to_string())?;
        let view = self
            .pairing
            .start_outgoing_pairing(&peer)
            .await
            .map_err(|e| e.to_string())?;
        self.publish_snapshot();
        Ok(view)
    }

    pub async fn approve_pair(&self, session_id: &str) -> Result<PendingPairView, String> {
        let cb: &dyn PeerCallbacks = self;
        let view = self
            .pairing
            .approve_local(session_id, Some(cb))
            .await
            .map_err(|e| e.to_string())?;
        if view.state == PairState::ApprovedLocal && view.is_initiator {
            let rt = self.clone();
            let sid = session_id.to_string();
            tokio::spawn(async move {
                for _ in 0..60 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let cb_ref: &dyn PeerCallbacks = &rt;
                    match rt.pairing.poll_outgoing_pair(&sid, Some(cb_ref)).await {
                        Ok(v) if !matches!(v.state, PairState::Pending | PairState::ApprovedLocal) => {
                            rt.publish_snapshot();
                            break;
                        }
                        Ok(_) => {}
                        Err(err) => {
                            debug!("poll_outgoing_pair finished/errored: {err}");
                            break;
                        }
                    }
                }
            });
        }
        self.publish_snapshot();
        Ok(view)
    }

    pub async fn reject_pair(&self, session_id: &str) -> Result<(), String> {
        self.pairing
            .reject_local(session_id)
            .await
            .map_err(|e| e.to_string())?;
        self.publish_snapshot();
        Ok(())
    }

    pub fn unpair_peer(&self, device_id: &str) -> Result<(), String> {
        self.trust.remove(device_id).map_err(|e| e.to_string())?;
        self.peer_status
            .write()
            .expect("peer_status lock poisoned")
            .remove(device_id);
        self.publish_snapshot();
        Ok(())
    }

    pub fn cancel_transfer(&self, batch_id: &str) {
        if let Some(tx) = self
            .active_cancels
            .read()
            .expect("active_cancels lock poisoned")
            .get(batch_id)
        {
            let _ = tx.send(true);
        }
    }

    fn upsert_transfer(&self, rec: TransferRecord) {
        let mut guard = self.transfers.write().expect("transfers lock poisoned");
        if let Some(existing) = guard.iter_mut().find(|r| r.id == rec.id) {
            *existing = rec;
        } else {
            guard.push_front(rec);
            while guard.len() > MAX_TRANSFER_HISTORY {
                guard.pop_back();
            }
        }
    }

    /// Stream one or more local files to a connected paired peer, publishing progress events.
    pub async fn send_files_to_peer(
        &self,
        peer_id: &str,
        paths: Vec<PathBuf>,
        progress_tx: Option<mpsc::UnboundedSender<IpcResponse>>,
    ) -> Result<TransferRecord, String> {
        if paths.is_empty() {
            return Err("未选择要发送的文件".to_string());
        }
        let trusted = self
            .trust
            .get_by_device_id(peer_id)
            .ok_or_else(|| "目标设备未配对".to_string())?;

        let discovered_ip = self
            .discovery
            .get_peer(peer_id)
            .map(|p| (p.ip, p.tls_port));
        let status_ip = self
            .peer_status
            .read()
            .expect("peer_status lock poisoned")
            .get(peer_id)
            .and_then(|s| s.ip.clone().map(|ip| (ip, s.tls_port)));

        let (ip, tls_port) = discovered_ip
            .or(status_ip)
            .or_else(|| trusted.last_known_ip.clone().map(|ip| (ip, trusted.tls_port)))
            .ok_or_else(|| "设备当前离线或未发现局域网 IP".to_string())?;

        let mut valid_files: Vec<(PathBuf, u64)> = Vec::new();
        let mut total_bytes = 0u64;
        for p in paths {
            let meta = tokio::fs::metadata(&p)
                .await
                .map_err(|e| format!("无法读取 {}: {e}", p.display()))?;
            if !meta.is_file() {
                return Err(format!("仅支持发送普通文件: {}", p.display()));
            }
            total_bytes = total_bytes.saturating_add(meta.len());
            valid_files.push((p, meta.len()));
        }

        let batch_id = Uuid::new_v4().to_string();
        let summary = if valid_files.len() == 1 {
            valid_files[0]
                .0
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        } else {
            let first = valid_files[0]
                .0
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            format!("{first} 等 {} 个文件", valid_files.len())
        };

        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.active_cancels
            .write()
            .expect("active_cancels lock poisoned")
            .insert(batch_id.clone(), cancel_tx);

        let mut target_peer = trusted.clone();
        target_peer.tls_port = tls_port;
        let file_total = valid_files.len();
        let mut completed_prefix_bytes = 0u64;

        let mut record = TransferRecord {
            id: batch_id.clone(),
            peer_id: trusted.device_id.clone(),
            peer_name: trusted.device_name.clone(),
            direction: TransferDirection::Send,
            summary: summary.clone(),
            file_count: file_total,
            bytes_transferred: 0,
            total_bytes,
            state: TransferState::Transferring,
            error_message: None,
            dest_path: None,
            updated_at_ms: now_timestamp_ms(),
        };
        self.upsert_transfer(record.clone());
        self.publish_snapshot();
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(IpcResponse::TransferProgress {
                record: record.clone(),
            });
        }

        let client = MtlsClient::new(self.identity.clone());
        let mut transfer_err: Option<String> = None;

        for (idx, (path, file_size)) in valid_files.into_iter().enumerate() {
            let rt_clone = self.clone();
            let batch_clone = batch_id.clone();
            let progress_tx_clone = progress_tx.clone();
            let base_rec = record.clone();

            let cb: UploadProgressFn = Arc::new(move |sent_in_file, _| {
                let total_done = completed_prefix_bytes.saturating_add(sent_in_file).min(total_bytes);
                let mut upd = base_rec.clone();
                upd.bytes_transferred = total_done;
                upd.updated_at_ms = now_timestamp_ms();
                rt_clone.upsert_transfer(upd.clone());
                rt_clone.publish_snapshot();
                if let Some(ref tx) = progress_tx_clone {
                    let _ = tx.send(IpcResponse::TransferProgress { record: upd });
                }
                let _ = &batch_clone;
            });

            match client
                .upload_file_path(
                    &target_peer,
                    &ip,
                    &path,
                    &batch_id,
                    idx + 1,
                    file_total,
                    Some(cb),
                    Some(cancel_rx.clone()),
                )
                .await
            {
                Ok(_) => {
                    completed_prefix_bytes = completed_prefix_bytes.saturating_add(file_size);
                    record.bytes_transferred = completed_prefix_bytes;
                    record.updated_at_ms = now_timestamp_ms();
                    self.upsert_transfer(record.clone());
                    self.publish_snapshot();
                }
                Err(err) => {
                    transfer_err = Some(err.to_string());
                    if matches!(err, workspace_core::mtls_http::NetError::Cancelled) {
                        record.state = TransferState::Cancelled;
                    } else {
                        record.state = TransferState::Failed;
                    }
                    record.error_message = transfer_err.clone();
                    record.updated_at_ms = now_timestamp_ms();
                    break;
                }
            }
        }

        self.active_cancels
            .write()
            .expect("active_cancels lock poisoned")
            .remove(&batch_id);

        if transfer_err.is_none() {
            record.bytes_transferred = total_bytes;
            record.state = TransferState::Completed;
            record.updated_at_ms = now_timestamp_ms();
            send_notification(
                format!("已发送到 {}", trusted.device_name),
                format!("{} ({})", summary, format_human_size(total_bytes)),
            );
        }

        self.upsert_transfer(record.clone());
        self.publish_snapshot();
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(IpcResponse::TransferFinished {
                record: record.clone(),
            });
        }

        match transfer_err {
            None => Ok(record),
            Some(err) => Err(err),
        }
    }
}

impl PeerCallbacks for DaemonRuntime {
    fn receive_dir(&self) -> PathBuf {
        self.config
            .read()
            .expect("config lock poisoned")
            .receive_dir
            .clone()
    }

    fn on_clipboard_push<'a>(
        &'a self,
        peer: &'a TrustedPeer,
        msg: ClipboardMessage,
        payload: ValidatedPayload,
    ) -> BoxFuture<'a, Result<ClipboardPushResponse, ApiErrorBody>> {
        Box::pin(async move {
            let paused = self.config.read().expect("config lock poisoned").sync_paused;
            if paused {
                return Err(ApiErrorBody::new(
                    ErrorCode::SyncPaused,
                    "Linux 端已暂停接收剪贴板同步",
                ));
            }
            let meta = SnapshotMetadata::from(&msg);
            let kind = msg.kind;
            let size = msg.size;
            let message_id = msg.message_id.clone();
            let checksum = msg.checksum.clone();

            match apply_remote_clipboard(msg, &payload, &self.dedup, &self.latest_snapshot).await {
                Ok((applied, deduplicated)) => {
                    if applied {
                        *self
                            .last_clipboard
                            .write()
                            .expect("last_clipboard lock poisoned") = Some(ClipboardSummary {
                            direction: TransferDirection::Receive,
                            peer_name: peer.device_name.clone(),
                            kind,
                            size,
                            timestamp_ms: now_timestamp_ms(),
                            snapshot: Some(meta),
                        });
                        send_notification(
                            format!("已接收来自 {} 的剪贴板", peer.device_name),
                            format!(
                                "{} · {}",
                                match kind {
                                    workspace_core::clipboard_model::ClipKind::Text => "文本",
                                    workspace_core::clipboard_model::ClipKind::Image => "图片",
                                },
                                format_human_size(size as u64)
                            ),
                        );
                        self.publish_snapshot();
                    }
                    Ok(ClipboardPushResponse {
                        ok: true,
                        applied,
                        deduplicated,
                        message_id,
                        checksum,
                    })
                }
                Err(err) => {
                    tracing::warn!(
                        "failed to apply incoming clipboard push from {}: {err}",
                        peer.device_name
                    );
                    Err(ApiErrorBody::new(ErrorCode::ClipboardWriteFailed, err))
                }
            }
        })
    }

    fn on_clipboard_pull<'a>(
        &'a self,
        peer: &'a TrustedPeer,
    ) -> BoxFuture<'a, Result<ClipboardMessage, ApiErrorBody>> {
        Box::pin(async move {
            let paused = self.config.read().expect("config lock poisoned").sync_paused;
            if paused {
                return Err(ApiErrorBody::new(
                    ErrorCode::SyncPaused,
                    "Linux 端已暂停剪贴板同步",
                ));
            }
            let origin = self.identity.device_id.clone();
            let seq = now_timestamp_ms();
            let msg = match read_wayland_clipboard(&origin, seq).await {
                Ok(m) => {
                    *self.latest_snapshot.write().await = Some(m.clone());
                    m
                }
                Err(err) => {
                    if let Some(cached) = self.latest_snapshot.read().await.clone() {
                        cached
                    } else {
                        return Err(ApiErrorBody::new(err.code(), err.to_string()));
                    }
                }
            };

            *self
                .last_clipboard
                .write()
                .expect("last_clipboard lock poisoned") = Some(ClipboardSummary {
                direction: TransferDirection::Send,
                peer_name: peer.device_name.clone(),
                kind: msg.kind,
                size: msg.size,
                timestamp_ms: now_timestamp_ms(),
                snapshot: Some(SnapshotMetadata::from(&msg)),
            });
            self.publish_snapshot();
            Ok(msg)
        })
    }

    fn on_file_receiving<'a>(
        &'a self,
        peer: &'a TrustedPeer,
        batch_id: &'a str,
        file_name: &'a str,
        file_index: usize,
        file_total: usize,
        bytes_received: u64,
        total_bytes: u64,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let rec = TransferRecord {
                id: batch_id.to_string(),
                peer_id: peer.device_id.clone(),
                peer_name: peer.device_name.clone(),
                direction: TransferDirection::Receive,
                summary: if file_total > 1 {
                    format!("{file_name} ({file_index}/{file_total})")
                } else {
                    file_name.to_string()
                },
                file_count: file_total,
                bytes_transferred: bytes_received,
                total_bytes,
                state: TransferState::Transferring,
                error_message: None,
                dest_path: None,
                updated_at_ms: now_timestamp_ms(),
            };
            self.upsert_transfer(rec);
            self.publish_snapshot();
        })
    }

    fn on_file_received<'a>(
        &'a self,
        peer: &'a TrustedPeer,
        batch_id: &'a str,
        file_index: usize,
        file_total: usize,
        received: ReceivedFile,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let done = file_index >= file_total;
            let rec = TransferRecord {
                id: batch_id.to_string(),
                peer_id: peer.device_id.clone(),
                peer_name: peer.device_name.clone(),
                direction: TransferDirection::Receive,
                summary: if file_total > 1 {
                    format!("{} ({file_index}/{file_total})", received.file_name)
                } else {
                    received.file_name.clone()
                },
                file_count: file_total,
                bytes_transferred: received.size,
                total_bytes: received.size,
                state: if done {
                    TransferState::Completed
                } else {
                    TransferState::Transferring
                },
                error_message: None,
                dest_path: Some(received.path.display().to_string()),
                updated_at_ms: now_timestamp_ms(),
            };
            self.upsert_transfer(rec);
            send_notification(
                format!("收到来自 {} 的文件", peer.device_name),
                format!(
                    "{} ({})",
                    received.file_name,
                    format_human_size(received.size)
                ),
            );
            self.publish_snapshot();
        })
    }

    fn on_pair_requested<'a>(&'a self, view: PendingPairView) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            send_notification(
                format!("收到来自 {} 的配对请求", view.peer_name),
                format!("请核对 6 位验证码：{}", view.sas_code),
            );
            self.request_show_window(None);
            self.publish_snapshot();
        })
    }

    fn on_pair_completed<'a>(&'a self, peer: TrustedPeer) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            send_notification("设备配对成功", format!("已信任：{}", peer.device_name));
            self.publish_snapshot();
        })
    }
}

pub struct StartedDaemon {
    pub runtime: DaemonRuntime,
    _server: MtlsServerHandle,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl StartedDaemon {
    pub async fn start(
        state_dir_override: Option<PathBuf>,
        action_tx: mpsc::UnboundedSender<TrayAction>,
    ) -> anyhow::Result<Self> {
        workspace_core::crypto::install_crypto_provider();
        let state_dir = state_dir_override.unwrap_or_else(default_state_dir);
        let cfg = load_or_create_config(&state_dir)?;
        let _ = std::fs::create_dir_all(&cfg.receive_dir);

        let identity = Arc::new(load_or_create_identity(
            &state_dir,
            &cfg.device_name,
            Platform::Linux,
        )?);
        if identity.device_name() != cfg.device_name {
            identity.set_device_name(cfg.device_name.clone());
            let _ = workspace_core::store::save_identity(&state_dir, &identity);
        }
        let trust = TrustRegistry::load_in_dir(&state_dir)?;
        let pairing = PairingManager::new(identity.clone(), trust.clone());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let (discovery, disc_task) = DiscoveryHandle::start(
            identity.clone(),
            trust.clone(),
            DiscoveryConfig {
                discovery_port: cfg.discovery_port,
                tls_port: cfg.tls_port,
                ..Default::default()
            },
            shutdown_rx.clone(),
        )
        .await?;

        let (ui_tx, _) = broadcast::channel(64);
        let (tray_tx, tray_rx) = watch::channel(TrayStatus {
            connected: false,
            peer_summary: "初始化中".to_string(),
            sync_paused: cfg.sync_paused,
        });

        let runtime = DaemonRuntime {
            state_dir: state_dir.clone(),
            identity: identity.clone(),
            trust: trust.clone(),
            pairing: pairing.clone(),
            discovery: discovery.clone(),
            config: Arc::new(RwLock::new(cfg.clone())),
            dedup: Arc::new(tokio::sync::Mutex::new(DedupTracker::new())),
            latest_snapshot: Arc::new(tokio::sync::RwLock::new(None)),
            peer_status: Arc::new(RwLock::new(HashMap::new())),
            last_clipboard: Arc::new(RwLock::new(None)),
            transfers: Arc::new(RwLock::new(VecDeque::new())),
            active_cancels: Arc::new(RwLock::new(HashMap::new())),
            ui_tx,
            tray_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let server = start_mtls_server(
            std::net::SocketAddr::from(([0, 0, 0, 0], cfg.tls_port)),
            identity.clone(),
            trust,
            pairing,
            Arc::new(runtime.clone()),
            TlsAuthPolicy::AllowPairingRequests,
            shutdown_rx.clone(),
        )
        .await?;
        info!(
            "寰宇工作台 mTLS 服务已在端口 {} 启动 (device_id={})",
            server.local_addr().port(),
            identity.device_id
        );

        let watch_task = spawn_clipboard_watcher(
            identity.device_id.clone(),
            runtime.dedup.clone(),
            runtime.latest_snapshot.clone(),
            shutdown_rx.clone(),
        );
        let tray_task = spawn_tray_service(action_tx, tray_rx, shutdown_rx.clone());
        let monitor_task = spawn_lan_monitor(runtime.clone(), shutdown_rx.clone());
        let ipc_task = spawn_unix_ipc(runtime.clone(), shutdown_rx)?;

        runtime.publish_snapshot();

        Ok(Self {
            runtime,
            _server: server,
            tasks: vec![disc_task, watch_task, tray_task, monitor_task, ipc_task],
        })
    }

    pub async fn wait_until_shutdown(self) {
        let mut rx = self.runtime.shutdown_tx.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

fn spawn_lan_monitor(
    rt: DaemonRuntime,
    mut shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut disc_rx = rt.discovery.subscribe();
        let mut tick = tokio::time::interval(MONITOR_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            probe_trusted_peers(&rt).await;
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = tick.tick() => {}
                _ = disc_rx.changed() => {
                    rt.publish_snapshot();
                }
            }
        }
    })
}

async fn probe_trusted_peers(rt: &DaemonRuntime) {
    let peers = rt.trust.list();
    let cfg_port = rt.config.read().expect("config lock poisoned").tls_port;
    let client = MtlsClient::new(rt.identity.clone());
    let mut changed = false;

    for tp in peers {
        let disc = rt.discovery.get_peer(&tp.device_id);
        let candidate_ip = disc
            .as_ref()
            .map(|d| d.ip.clone())
            .or_else(|| tp.last_known_ip.clone());
        let target_port = disc.as_ref().map(|d| d.tls_port).unwrap_or(tp.tls_port);

        let mut target = tp.clone();
        target.tls_port = target_port;

        let (connected, rtt_ms, active_ip, live_name) = if let Some(ref ip) = candidate_ip {
            let t0 = Instant::now();
            match client.heartbeat(&target, ip, cfg_port).await {
                Ok(resp) => {
                    let updated_name = if resp.device_id == tp.device_id
                        && !resp.device_name.trim().is_empty()
                    {
                        let _ = rt.trust.update_seen_endpoint(
                            &tp.device_id,
                            Some(ip),
                            target_port,
                            Some(&resp.device_name),
                        );
                        resp.device_name
                    } else {
                        disc.as_ref()
                            .map(|d| d.device_name.clone())
                            .unwrap_or_else(|| tp.device_name.clone())
                    };
                    (
                        true,
                        Some(t0.elapsed().as_millis() as u64),
                        Some(ip.clone()),
                        updated_name,
                    )
                }
                Err(_) => (
                    false,
                    None,
                    Some(ip.clone()),
                    disc.as_ref()
                        .map(|d| d.device_name.clone())
                        .unwrap_or_else(|| tp.device_name.clone()),
                ),
            }
        } else {
            (
                false,
                None,
                None,
                disc.as_ref()
                    .map(|d| d.device_name.clone())
                    .unwrap_or_else(|| tp.device_name.clone()),
            )
        };

        let fp = workspace_core::crypto::decode_pubkey_b64(&tp.pubkey_b64)
            .map(|pk| workspace_core::crypto::format_pubkey_fingerprint(&pk))
            .unwrap_or_default();

        let prev_connected = rt
            .peer_status
            .read()
            .expect("peer_status lock poisoned")
            .get(&tp.device_id)
            .map(|s| s.connected)
            .unwrap_or(false);

        if prev_connected != connected {
            changed = true;
            if connected {
                info!("局域网设备已连接: {} ({:?})", live_name, active_ip);
                send_notification(
                    format!("{} 已进入同一局域网", live_name),
                    "寰宇工作台互传与剪贴板控制已就绪",
                );
            } else {
                info!("设备离开局域网: {}", live_name);
            }
        }

        let new_status = PairedPeerStatus {
            device_id: tp.device_id.clone(),
            device_name: live_name,
            platform: tp.platform,
            fingerprint: fp,
            connected,
            ip: active_ip,
            tls_port: target_port,
            rtt_ms,
            last_seen_ms: if connected {
                Some(now_timestamp_ms())
            } else {
                rt.peer_status
                    .read()
                    .expect("peer_status lock poisoned")
                    .get(&tp.device_id)
                    .and_then(|s| s.last_seen_ms)
            },
        };

        let mut guard = rt.peer_status.write().expect("peer_status lock poisoned");
        if guard.get(&tp.device_id) != Some(&new_status) {
            changed = true;
            guard.insert(tp.device_id, new_status);
        }
    }

    if changed {
        rt.publish_snapshot();
    }
}

fn spawn_unix_ipc(
    rt: DaemonRuntime,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let sock_path = runtime_socket_path();
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path)?;
    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))?;

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                accept_res = listener.accept() => {
                    let (stream, _) = match accept_res {
                        Ok(s) => s,
                        Err(err) => {
                            warn!("Unix IPC accept error: {err}");
                            continue;
                        }
                    };
                    let rt_conn = rt.clone();
                    tokio::spawn(async move {
                        handle_ipc_client(rt_conn, stream).await;
                    });
                }
            }
        }
        let _ = std::fs::remove_file(&sock_path);
    });
    Ok(join)
}

async fn handle_ipc_client(rt: DaemonRuntime, stream: UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let req: IpcRequest = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(err) => {
            let _ = write_ipc_line(
                &mut writer,
                &IpcResponse::Error {
                    message: format!("invalid IPC JSON: {err}"),
                },
            )
            .await;
            return;
        }
    };

    match req {
        IpcRequest::ShowWindow { activation_token } => {
            rt.request_show_window(activation_token);
            let _ = write_ipc_line(&mut writer, &IpcResponse::Ok).await;
        }
        IpcRequest::GetState => {
            let _ = write_ipc_line(
                &mut writer,
                &IpcResponse::State {
                    snapshot: Box::new(rt.snapshot()),
                },
            )
            .await;
        }
        IpcRequest::StartPair { device_id } => {
            let resp = match rt.start_pair(&device_id).await {
                Ok(session) => IpcResponse::PairSession { session },
                Err(message) => IpcResponse::Error { message },
            };
            let _ = write_ipc_line(&mut writer, &resp).await;
        }
        IpcRequest::ApprovePair { session_id } => {
            let resp = match rt.approve_pair(&session_id).await {
                Ok(session) => IpcResponse::PairSession { session },
                Err(message) => IpcResponse::Error { message },
            };
            let _ = write_ipc_line(&mut writer, &resp).await;
        }
        IpcRequest::RejectPair { session_id } => {
            let resp = match rt.reject_pair(&session_id).await {
                Ok(()) => IpcResponse::Ok,
                Err(message) => IpcResponse::Error { message },
            };
            let _ = write_ipc_line(&mut writer, &resp).await;
        }
        IpcRequest::Unpair { device_id } => {
            let resp = match rt.unpair_peer(&device_id) {
                Ok(()) => IpcResponse::Ok,
                Err(message) => IpcResponse::Error { message },
            };
            let _ = write_ipc_line(&mut writer, &resp).await;
        }
        IpcRequest::SetSyncPaused { paused } => {
            let resp = match rt.set_sync_paused(paused) {
                Ok(()) => IpcResponse::Ok,
                Err(message) => IpcResponse::Error { message },
            };
            let _ = write_ipc_line(&mut writer, &resp).await;
        }
        IpcRequest::CancelTransfer { batch_id } => {
            rt.cancel_transfer(&batch_id);
            let _ = write_ipc_line(&mut writer, &IpcResponse::Ok).await;
        }
        IpcRequest::Quit => {
            let _ = write_ipc_line(&mut writer, &IpcResponse::Ok).await;
            rt.request_quit();
        }
        IpcRequest::SendFiles {
            peer_id,
            paths,
            stream: stream_progress,
        } => {
            if !stream_progress {
                let resp = match rt.send_files_to_peer(&peer_id, paths, None).await {
                    Ok(record) => IpcResponse::TransferFinished { record },
                    Err(message) => IpcResponse::Error { message },
                };
                let _ = write_ipc_line(&mut writer, &resp).await;
                return;
            }

            let (tx, mut rx) = mpsc::unbounded_channel();
            let rt_send = rt.clone();
            let send_task = tokio::spawn(async move {
                rt_send.send_files_to_peer(&peer_id, paths, Some(tx)).await
            });

            // Forward progress lines while watching for cancel request on client stdin.
            let rt_cancel = rt.clone();
            let cancel_reader = tokio::spawn(async move {
                while let Ok(Some(cmd_line)) = lines.next_line().await {
                    if let Ok(IpcRequest::CancelTransfer { batch_id }) =
                        serde_json::from_str::<IpcRequest>(&cmd_line)
                    {
                        rt_cancel.cancel_transfer(&batch_id);
                    }
                }
            });

            while let Some(ev) = rx.recv().await {
                if write_ipc_line(&mut writer, &ev).await.is_err() {
                    break;
                }
                if matches!(ev, IpcResponse::TransferFinished { .. }) {
                    break;
                }
            }
            cancel_reader.abort();
            let _ = send_task.await;
        }
    }
}

async fn write_ipc_line(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    resp: &IpcResponse,
) -> std::io::Result<()> {
    let mut encoded = serde_json::to_string(resp).unwrap_or_else(|_| "{}".to_string());
    encoded.push('\n');
    writer.write_all(encoded.as_bytes()).await?;
    writer.flush().await
}

/// Helper for CLI & secondary instances to send a single request to the running daemon.
pub async fn send_ipc_once(req: &IpcRequest) -> Result<IpcResponse, String> {
    let sock_path = runtime_socket_path();
    let stream = UnixStream::connect(&sock_path)
        .await
        .map_err(|e| format!("无法连接到正在运行的 寰宇工作台 后台 ({}): {e}", sock_path.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;

    let mut lines = BufReader::new(reader).lines();
    let resp_line = lines
        .next_line()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "daemon closed IPC socket".to_string())?;
    serde_json::from_str(&resp_line).map_err(|e| e.to_string())
}

pub fn open_folder_xdg(path: &Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
