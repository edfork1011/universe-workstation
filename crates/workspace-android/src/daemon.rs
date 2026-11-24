//! Core Android root daemon (`workspaced`): mTLS server, LAN monitor, clipboard control,
//! OShare LAN streaming proxy, and received file handler (`/storage/emulated/0/Download/寰宇工作台`).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};
use workspace_core::clipboard_model::{
    ClipKind, ClipboardMessage, SnapshotMetadata, ValidatedPayload, now_timestamp_ms,
};
use workspace_core::crypto::DeviceIdentity;
use workspace_core::discovery::{DiscoveryConfig, DiscoveryHandle};
use workspace_core::error::{ApiErrorBody, ErrorCode};
use workspace_core::limits::{PROTOCOL_VERSION, format_human_size};
use workspace_core::mtls_http::{
    FileUploadMeta, MtlsClient, MtlsServerHandle, PairingManager, PeerCallbacks, TlsAuthPolicy,
    UploadProgressFn, start_mtls_server,
};
use workspace_core::protocol::{
    ClipboardPushResponse, ClipboardSummary, DiscoveredPeer, PairedPeerStatus, PairState,
    PendingPairView, Platform, TransferDirection, TransferRecord, TransferState, TrustedPeer,
};
use workspace_core::store::{TrustRegistry, load_or_create_identity};
use workspace_core::transfer::ReceivedFile;

use crate::bridge::AppProcessBridge;
use crate::config::{
    AndroidConfig, default_state_dir, load_or_create_config, save_config,
};

const MAX_TRANSFER_HISTORY: usize = 32;
const MONITOR_INTERVAL: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidDaemonSnapshot {
    pub protocol: u32,
    pub device_id: String,
    pub device_name: String,
    pub device_name_customized: bool,
    pub user_name: String,
    pub fingerprint: String,
    pub tls_port: u16,
    pub discovery_port: u16,
    pub local_ipc_port: u16,
    pub lan_connected: bool,
    pub sync_paused: bool,
    pub gesture_enabled: bool,
    pub gesture_fingers: u8,
    pub swipe_up_action: String,
    pub swipe_down_action: String,
    pub receive_dir: String,
    pub paired_peers: Vec<PairedPeerStatus>,
    pub discovered_peers: Vec<DiscoveredPeer>,
    pub pending_pairs: Vec<PendingPairView>,
    pub last_clipboard: Option<ClipboardSummary>,
    pub transfers: Vec<TransferRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidClipboardActionResult {
    pub ok: bool,
    pub action: String,
    pub peer_name: String,
    pub kind: ClipKind,
    pub size: usize,
    pub message: String,
}

#[derive(Clone)]
pub struct AndroidDaemonRuntime {
    pub state_dir: PathBuf,
    pub identity: Arc<DeviceIdentity>,
    pub trust: TrustRegistry,
    pub pairing: PairingManager,
    pub discovery: DiscoveryHandle,
    pub bridge: AppProcessBridge,
    pub config: Arc<RwLock<AndroidConfig>>,
    peer_status: Arc<RwLock<HashMap<String, PairedPeerStatus>>>,
    last_clipboard: Arc<RwLock<Option<ClipboardSummary>>>,
    transfers: Arc<RwLock<VecDeque<TransferRecord>>>,
    active_cancels: Arc<RwLock<HashMap<String, watch::Sender<bool>>>>,
    events_tx: broadcast::Sender<AndroidDaemonSnapshot>,
    shutdown_tx: watch::Sender<bool>,
}

impl AndroidDaemonRuntime {
    pub fn snapshot(&self) -> AndroidDaemonSnapshot {
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

        let lan_connected = paired_peers.iter().any(|p| p.connected);
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

        AndroidDaemonSnapshot {
            protocol: PROTOCOL_VERSION,
            device_id: self.identity.device_id.clone(),
            device_name: self.identity.device_name(),
            device_name_customized: cfg.device_name_customized,
            user_name: cfg.user_name.clone(),
            fingerprint: self.identity.fingerprint(),
            tls_port: cfg.tls_port,
            discovery_port: cfg.discovery_port,
            local_ipc_port: cfg.local_ipc_port,
            lan_connected,
            sync_paused: cfg.sync_paused,
            gesture_enabled: cfg.gesture_enabled,
            gesture_fingers: cfg.gesture_fingers,
            swipe_up_action: cfg.swipe_up_action,
            swipe_down_action: cfg.swipe_down_action,
            receive_dir: cfg.receive_dir.display().to_string(),
            paired_peers,
            discovered_peers,
            pending_pairs,
            last_clipboard,
            transfers,
        }
    }

    pub fn publish_snapshot(&self) {
        let _ = self.events_tx.send(self.snapshot());
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AndroidDaemonSnapshot> {
        self.events_tx.subscribe()
    }

    pub fn update_config<F: FnOnce(&mut AndroidConfig)>(&self, edit: F) -> Result<(), String> {
        {
            let mut cfg = self.config.write().expect("config lock poisoned");
            edit(&mut cfg);
            save_config(&self.state_dir, &cfg).map_err(|e| e.to_string())?;
        }
        self.publish_snapshot();
        Ok(())
    }

    pub async fn apply_identity_name_change(&self, new_name: String) {
        self.identity.set_device_name(new_name);
        let _ = workspace_core::store::save_identity(&self.state_dir, &self.identity);
        self.discovery.announce_now().await;
        self.publish_snapshot();
    }

    pub async fn start_pair(&self, device_id: &str) -> Result<PendingPairView, String> {
        let peer = self
            .discovery
            .get_peer(device_id)
            .ok_or_else(|| "未在当前局域网发现该电脑，请确认处于同一 Wi-Fi".to_string())?;
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
                            debug!("android poll_outgoing_pair ended: {err}");
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

    /// Resolve a connected paired peer endpoint `(TrustedPeer, ip)`.
    pub fn resolve_target_peer(&self, peer_id: Option<&str>) -> Result<(TrustedPeer, String), String> {
        let snap = self.snapshot();
        let chosen_status = if let Some(pid) = peer_id {
            snap.paired_peers
                .iter()
                .find(|p| p.device_id == pid)
                .cloned()
        } else {
            snap.paired_peers
                .iter()
                .find(|p| p.connected)
                .cloned()
                .or_else(|| snap.paired_peers.first().cloned())
        };

        let status = chosen_status.ok_or_else(|| "暂无已配对的电脑设备".to_string())?;
        if !status.connected {
            return Err(format!("设备 {} 当前未在同一局域网连接", status.device_name));
        }
        let mut trusted = self
            .trust
            .get_by_device_id(&status.device_id)
            .ok_or_else(|| "配对信息丢失".to_string())?;
        let ip = status
            .ip
            .or_else(|| self.discovery.get_peer(&trusted.device_id).map(|d| d.ip))
            .ok_or_else(|| "无法解析目标设备局域网 IP".to_string())?;
        trusted.tls_port = status.tls_port;
        Ok((trusted, ip))
    }

    /// Android -> Linux clipboard PUSH (strictly Android-controlled).
    pub async fn push_clipboard_to_peer(
        &self,
        peer_id: Option<&str>,
    ) -> Result<AndroidClipboardActionResult, String> {
        if self.config.read().expect("config lock poisoned").sync_paused {
            return Err("剪贴板同步已暂停".to_string());
        }
        let (peer, ip) = self.resolve_target_peer(peer_id)?;
        let seq = now_timestamp_ms();
        let msg = match self
            .bridge
            .read_clipboard(&self.identity.device_id, seq)
            .await
        {
            Ok(m) => m,
            Err(err) => {
                warn!("clipboard push aborted while reading Android clipboard: {err}");
                return Err(err);
            }
        };

        let kind = msg.kind;
        let size = msg.size;
        let meta = SnapshotMetadata::from(&msg);
        let client = MtlsClient::new(self.identity.clone());
        client.push_clipboard(&peer, &ip, &msg).await.map_err(|e| {
            let err_msg = format!("推送到 {} 失败: {e}", peer.device_name);
            warn!("{err_msg}");
            err_msg
        })?;

        *self
            .last_clipboard
            .write()
            .expect("last_clipboard lock poisoned") = Some(ClipboardSummary {
            direction: TransferDirection::Send,
            peer_name: peer.device_name.clone(),
            kind,
            size,
            timestamp_ms: now_timestamp_ms(),
            snapshot: Some(meta),
        });
        self.publish_snapshot();

        Ok(AndroidClipboardActionResult {
            ok: true,
            action: "push".to_string(),
            peer_name: peer.device_name,
            kind,
            size,
            message: format!("已发送剪贴板 ({})", format_human_size(size as u64)),
        })
    }

    /// Android <- Linux clipboard PULL (strictly Android-controlled).
    pub async fn pull_clipboard_from_peer(
        &self,
        peer_id: Option<&str>,
    ) -> Result<AndroidClipboardActionResult, String> {
        if self.config.read().expect("config lock poisoned").sync_paused {
            return Err("剪贴板同步已暂停".to_string());
        }
        let (peer, ip) = self.resolve_target_peer(peer_id)?;
        let client = MtlsClient::new(self.identity.clone());
        let msg = client.pull_clipboard(&peer, &ip).await.map_err(|e| {
            let err_msg = format!("从 {} 拉取剪贴板失败: {e}", peer.device_name);
            warn!("{err_msg}");
            err_msg
        })?;
        let payload = msg.validate_and_decode().map_err(|e| {
            let err_msg = e.to_string();
            warn!("clipboard pull validation failed from {}: {err_msg}", peer.device_name);
            err_msg
        })?;
        self.bridge.write_clipboard(&payload).await.map_err(|e| {
            warn!("clipboard pull write_clipboard failed from {}: {e}", peer.device_name);
            e
        })?;

        let kind = msg.kind;
        let size = msg.size;
        let meta = SnapshotMetadata::from(&msg);
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
        self.publish_snapshot();

        Ok(AndroidClipboardActionResult {
            ok: true,
            action: "pull".to_string(),
            peer_name: peer.device_name,
            kind,
            size,
            message: format!("已拉取剪贴板 ({})", format_human_size(size as u64)),
        })
    }

    /// Stream an OShare file directly over LAN mTLS to the selected paired PC.
    pub async fn proxy_oshare_stream<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
        &self,
        peer_id: &str,
        meta: FileUploadMeta,
        reader: R,
    ) -> Result<workspace_core::protocol::FileUploadResponse, String> {
        let (peer, ip) = self.resolve_target_peer(Some(peer_id))?;
        let batch_id = meta.batch_id;
        let file_name = meta.file_name;
        let total_size = meta.total_size;
        let file_index = meta.file_index;
        let file_total = meta.file_total;
        let cancel_rx = {
            let mut map = self
                .active_cancels
                .write()
                .expect("active_cancels lock poisoned");
            map.entry(batch_id.clone())
                .or_insert_with(|| watch::channel(false).0)
                .subscribe()
        };

        let summary = if file_total > 1 {
            format!("{file_name} ({file_index}/{file_total})")
        } else {
            file_name.clone()
        };

        let rec = TransferRecord {
            id: batch_id.clone(),
            peer_id: peer.device_id.clone(),
            peer_name: peer.device_name.clone(),
            direction: TransferDirection::Send,
            summary: summary.clone(),
            file_count: file_total,
            bytes_transferred: 0,
            total_bytes: total_size,
            state: TransferState::Transferring,
            error_message: None,
            dest_path: None,
            updated_at_ms: now_timestamp_ms(),
        };
        self.upsert_transfer(rec.clone());
        self.publish_snapshot();

        let rt_clone = self.clone();
        let rec_base = rec.clone();
        let on_progress: UploadProgressFn = Arc::new(move |sent, total| {
            let mut upd = rec_base.clone();
            upd.bytes_transferred = sent;
            upd.total_bytes = total;
            upd.updated_at_ms = now_timestamp_ms();
            rt_clone.upsert_transfer(upd);
            rt_clone.publish_snapshot();
        });

        let client = MtlsClient::new(self.identity.clone());
        let res = client
            .upload_reader(
                &peer,
                &ip,
                reader,
                FileUploadMeta {
                    batch_id: batch_id.clone(),
                    file_name: file_name.clone(),
                    total_size,
                    file_index,
                    file_total,
                    blake3_hex: None,
                },
                Some(on_progress),
                Some(cancel_rx),
            )
            .await;

        if file_index >= file_total || res.is_err() {
            self.active_cancels
                .write()
                .expect("active_cancels lock poisoned")
                .remove(&batch_id);
        }

        match res {
            Ok(ack) => {
                let mut done = rec;
                done.bytes_transferred = total_size;
                done.state = if file_index >= file_total {
                    TransferState::Completed
                } else {
                    TransferState::Transferring
                };
                done.updated_at_ms = now_timestamp_ms();
                self.upsert_transfer(done);
                self.publish_snapshot();
                Ok(ack)
            }
            Err(err) => {
                let mut failed = rec;
                failed.state = if matches!(err, workspace_core::mtls_http::NetError::Cancelled) {
                    TransferState::Cancelled
                } else {
                    TransferState::Failed
                };
                failed.error_message = Some(err.to_string());
                failed.updated_at_ms = now_timestamp_ms();
                self.upsert_transfer(failed);
                self.publish_snapshot();
                Err(err.to_string())
            }
        }
    }

    pub fn cancel_oshare(&self, batch_or_peer_id: &str) {
        if let Some(tx) = self
            .active_cancels
            .read()
            .expect("active_cancels lock poisoned")
            .get(batch_or_peer_id)
        {
            let _ = tx.send(true);
            return;
        }
        // Also cancel any active transfer matching peer_id if called from OShare cancelTask(target).
        let matching_ids: Vec<String> = self
            .transfers
            .read()
            .expect("transfers lock poisoned")
            .iter()
            .filter(|t| {
                t.peer_id == batch_or_peer_id && t.state == TransferState::Transferring
            })
            .map(|t| t.id.clone())
            .collect();
        let cancels = self
            .active_cancels
            .read()
            .expect("active_cancels lock poisoned");
        for id in matching_ids {
            if let Some(tx) = cancels.get(&id) {
                let _ = tx.send(true);
            }
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

    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl PeerCallbacks for AndroidDaemonRuntime {
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
                    "Android 端已暂停剪贴板接收",
                ));
            }
            self.bridge
                .write_clipboard(&payload)
                .await
                .map_err(|e| {
                    warn!("incoming clipboard push write_clipboard failed from {}: {e}", peer.device_name);
                    ApiErrorBody::new(ErrorCode::ClipboardWriteFailed, e)
                })?;

            *self
                .last_clipboard
                .write()
                .expect("last_clipboard lock poisoned") = Some(ClipboardSummary {
                direction: TransferDirection::Receive,
                peer_name: peer.device_name.clone(),
                kind: msg.kind,
                size: msg.size,
                timestamp_ms: now_timestamp_ms(),
                snapshot: Some(SnapshotMetadata::from(&msg)),
            });
            self.publish_snapshot();
            Ok(ClipboardPushResponse {
                ok: true,
                applied: true,
                deduplicated: false,
                message_id: msg.message_id,
                checksum: msg.checksum,
            })
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
                    "Android 端已暂停剪贴板读取",
                ));
            }
            let msg = self
                .bridge
                .read_clipboard(&self.identity.device_id, now_timestamp_ms())
                .await
                .map_err(|e| {
                    warn!("incoming clipboard pull read_clipboard failed for {}: {e}", peer.device_name);
                    ApiErrorBody::new(ErrorCode::ClipboardReadFailed, e)
                })?;
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
            self.bridge.scan_media(&received.path).await;
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
            self.publish_snapshot();
        })
    }

    fn on_pair_requested<'a>(&'a self, view: PendingPairView) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.bridge.notify_system(
                &format!("收到来自 {} 的配对请求", view.peer_name),
                &format!("请打开 寰宇工作台 App 核对 6 位验证码：{}", view.sas_code),
            );
            self.publish_snapshot();
        })
    }

    fn on_pair_completed<'a>(&'a self, peer: TrustedPeer) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.bridge.notify_system(
                "设备配对成功",
                &format!("已信任电脑：{}", peer.device_name),
            );
            self.publish_snapshot();
        })
    }
}

pub struct StartedAndroidDaemon {
    pub runtime: AndroidDaemonRuntime,
    _server: MtlsServerHandle,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl StartedAndroidDaemon {
    pub async fn start(state_dir_override: Option<PathBuf>) -> anyhow::Result<Self> {
        workspace_core::crypto::install_crypto_provider();
        let state_dir = state_dir_override.unwrap_or_else(default_state_dir);
        let cfg = load_or_create_config(&state_dir)?;
        let _ = std::fs::create_dir_all(&cfg.receive_dir);

        let identity = Arc::new(load_or_create_identity(
            &state_dir,
            &cfg.device_name,
            Platform::Android,
        )?);
        if identity.device_name() != cfg.device_name {
            identity.set_device_name(cfg.device_name.clone());
            let _ = workspace_core::store::save_identity(&state_dir, &identity);
        }
        let trust = TrustRegistry::load_in_dir(&state_dir)?;
        let pairing = PairingManager::new(identity.clone(), trust.clone());
        let bridge = AppProcessBridge::new(cfg.bridge_jar.clone(), cfg.staging_dir.clone());
        bridge.ensure_staging_dir();
        if unsafe { libc::geteuid() } == 0 {
            bridge.ensure_shell_clipboard_appops().await;
        }

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

        let (events_tx, _) = broadcast::channel(64);
        let runtime = AndroidDaemonRuntime {
            state_dir,
            identity: identity.clone(),
            trust: trust.clone(),
            pairing: pairing.clone(),
            discovery,
            bridge,
            config: Arc::new(RwLock::new(cfg.clone())),
            peer_status: Arc::new(RwLock::new(HashMap::new())),
            last_clipboard: Arc::new(RwLock::new(None)),
            transfers: Arc::new(RwLock::new(VecDeque::new())),
            active_cancels: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
            shutdown_tx,
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

        let local_ipc_task =
            crate::local_ipc::start_local_ipc_server(runtime.clone(), cfg.local_ipc_port, shutdown_rx.clone())
                .await?;
        let monitor_task = spawn_lan_monitor(runtime.clone(), shutdown_rx);

        info!(
            "workspaced 启动完成 (mTLS :{}, local IPC :{}, device_id={})",
            server.local_addr().port(),
            cfg.local_ipc_port,
            identity.device_id
        );

        runtime.publish_snapshot();

        Ok(Self {
            runtime,
            _server: server,
            tasks: vec![disc_task, local_ipc_task, monitor_task],
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
    rt: AndroidDaemonRuntime,
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

async fn probe_trusted_peers(rt: &AndroidDaemonRuntime) {
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
