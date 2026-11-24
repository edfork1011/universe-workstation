//! Network, pairing, session, and transfer wire types for 寰宇工作台 (`me.edfork.workspace`).

use serde::{Deserialize, Serialize};

use crate::clipboard_model::{ClipKind, SnapshotMetadata};

/// Supported device operating systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Android,
}

impl Platform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Android => "android",
        }
    }
}

/// Signed UDP LAN discovery beacon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryBeacon {
    pub protocol: u32,
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub tls_port: u16,
    pub pubkey_b64: String,
    pub timestamp_ms: u64,
    pub signature_b64: String,
}

/// Discovered device currently visible on the LAN (may be paired or unpaired).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub ip: String,
    pub tls_port: u16,
    pub pubkey_b64: String,
    pub fingerprint: String,
    pub is_trusted: bool,
    pub last_seen_ms: u64,
}

/// Persisted trusted (paired) peer record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub pubkey_b64: String,
    pub cert_sha256_hex: String,
    pub paired_at_ms: u64,
    pub last_known_ip: Option<String>,
    pub tls_port: u16,
}

/// Live status of a paired peer monitored in the background.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedPeerStatus {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub fingerprint: String,
    pub connected: bool,
    pub ip: Option<String>,
    pub tls_port: u16,
    pub rtt_ms: Option<u64>,
    pub last_seen_ms: Option<u64>,
}

/// Pairing state machine status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairState {
    Pending,
    ApprovedLocal,
    Completed,
    Rejected,
    Expired,
}

/// Active SAS pairing session view exposed to UI / IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPairView {
    pub session_id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub peer_platform: Platform,
    pub peer_pubkey_b64: String,
    pub peer_fingerprint: String,
    pub peer_ip: String,
    pub peer_tls_port: u16,
    pub sas_code: String,
    pub is_initiator: bool,
    pub state: PairState,
    pub expires_at_ms: u64,
}

/// Initiator -> Responder: initiate SAS pairing over TLS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequestPayload {
    pub protocol: u32,
    pub session_id: String,
    pub initiator_id: String,
    pub initiator_name: String,
    pub initiator_platform: Platform,
    pub initiator_pubkey_b64: String,
    pub initiator_tls_port: u16,
    pub initiator_nonce_b64: String,
    pub signature_b64: String,
}

/// Responder -> Initiator: accept pairing challenge and supply responder nonce + SAS signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequestResponse {
    pub ok: bool,
    pub session_id: String,
    pub responder_id: String,
    pub responder_name: String,
    pub responder_platform: Platform,
    pub responder_pubkey_b64: String,
    pub responder_nonce_b64: String,
    pub sas_code: String,
    pub signature_b64: String,
}

/// Initiator -> Responder: user confirmed SAS matches; deliver initiator's transcript signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairConfirmPayload {
    pub session_id: String,
    pub initiator_id: String,
    pub initiator_pubkey_b64: String,
    pub confirm_signature_b64: String,
}

/// Responder -> Initiator: pairing status / final responder confirmation signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairStatusResponse {
    pub ok: bool,
    pub session_id: String,
    pub state: PairState,
    pub responder_confirm_signature_b64: Option<String>,
}

/// Authenticated mTLS session heartbeat payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub protocol: u32,
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub tls_port: u16,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub ok: bool,
    pub protocol: u32,
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub tls_port: u16,
    pub timestamp_ms: u64,
}

/// Clipboard push acknowledgement returned by `POST /v1/clipboard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardPushResponse {
    pub ok: bool,
    pub applied: bool,
    pub deduplicated: bool,
    pub message_id: String,
    pub checksum: String,
}

/// File upload acknowledgement returned by `POST /v1/files/upload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileUploadResponse {
    pub ok: bool,
    pub batch_id: String,
    pub file_name: String,
    pub saved_path: String,
    pub size: u64,
    pub checksum: String,
}

/// Direction of a LAN file transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Send,
    Receive,
}

/// Lifecycle state of a LAN file transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Transferring,
    Completed,
    Failed,
    Cancelled,
}

/// Recent transfer status item displayed in UI & IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferRecord {
    pub id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub direction: TransferDirection,
    pub summary: String,
    pub file_count: usize,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub state: TransferState,
    pub error_message: Option<String>,
    pub dest_path: Option<String>,
    pub updated_at_ms: u64,
}

/// Summary of the most recent clipboard sync event (privacy-safe: no raw clipboard text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardSummary {
    pub direction: TransferDirection,
    pub peer_name: String,
    pub kind: ClipKind,
    pub size: usize,
    pub timestamp_ms: u64,
    pub snapshot: Option<SnapshotMetadata>,
}
