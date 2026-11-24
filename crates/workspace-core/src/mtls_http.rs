//! Mutual-TLS 1.3 HTTPS server & streaming client (`hyper` + `axum` + `tokio-rustls`).
//!
//! Enforces strict Ed25519 key pinning and TrustRegistry checks on every operational route:
//! unpaired or revoked peers are rejected with `403 UntrustedPeer` before touching
//! clipboard or disk state.

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Extension, Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use http::Request;
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::client::conn::http1;
use hyper::server::conn::http1 as server_http1;
use hyper_util::rt::TokioIo;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_util::io::ReaderStream;
use tower::ServiceExt;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::clipboard_model::{ClipboardMessage, ValidatedPayload, now_timestamp_ms};
use crate::crypto::{
    CryptoError, DeviceIdentity, build_mtls_client_config, build_mtls_server_config,
    build_mtls_server_config_with_filter, compute_sas_code, decode_pubkey_b64, derive_device_id,
    encode_pubkey_b64, extract_ed25519_pubkey_from_cert, format_pubkey_fingerprint,
    pair_confirm_transcript, pair_request_transcript, pair_response_transcript, sha256_hex,
    verify_ed25519_signature, workspace_server_name,
};
use crate::error::{ApiErrorBody, ErrorCode};
use crate::limits::{MAX_HTTP_BODY_BYTES, PROTOCOL_VERSION};
use crate::protocol::{
    ClipboardPushResponse, DiscoveredPeer, FileUploadResponse, HeartbeatRequest, HeartbeatResponse,
    PairConfirmPayload, PairRequestPayload, PairRequestResponse, PairState, PairStatusResponse,
    PendingPairView, Platform, TrustedPeer,
};
use crate::store::{StoreError, TrustRegistry};
use crate::transfer::{FileReceiver, ReceivedFile, percent_decode_utf8, percent_encode_utf8};

const PAIR_SESSION_TTL_MS: u64 = 120_000;
const MAX_ACTIVE_PAIR_SESSIONS: usize = 8;
const STREAM_CHUNK_SIZE: usize = 64 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(64);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const JSON_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PRE_HASH_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

pub const HDR_BATCH_ID: &str = "x-workspace-batch-id";
pub const HDR_FILE_NAME: &str = "x-workspace-file-name";
pub const HDR_FILE_SIZE: &str = "x-workspace-file-size";
pub const HDR_FILE_INDEX: &str = "x-workspace-file-index";
pub const HDR_FILE_TOTAL: &str = "x-workspace-file-total";
pub const HDR_FILE_BLAKE3: &str = "x-workspace-file-blake3";

#[derive(Debug, Error)]
pub enum NetError {
    #[error("network I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS or crypto error: {0}")]
    Crypto(#[from] CryptoError),
    #[error("HTTP protocol error: {0}")]
    Http(#[from] hyper::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("trust store error: {0}")]
    Store(#[from] StoreError),
    #[error("request timed out")]
    Timeout,
    #[error("remote peer returned HTTP {status}: {body}")]
    RemoteStatus { status: StatusCode, body: String },
    #[error("transfer cancelled")]
    Cancelled,
    #[error("invalid response or peer claim: {0}")]
    Protocol(String),
}

/// Verified Ed25519 identity extracted from an incoming TLS 1.3 client certificate.
#[derive(Debug, Clone)]
pub struct ClientCertInfo {
    pub pubkey: [u8; 32],
    pub pubkey_b64: String,
    pub device_id: String,
    pub cert_sha256_hex: String,
}

/// Controls whether non-paired client certs may open TLS connections to reach `/v1/pair/*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsAuthPolicy {
    /// Allow valid Ed25519 certs at TLS level so `/v1/pair/*` works; operational routes enforce `TrustRegistry`.
    AllowPairingRequests,
    /// Reject any unpaired peer already during the TLS 1.3 handshake.
    TrustedOnly,
}

/// Callback hooks implemented by platform daemons (`workspace-linux` / `workspace-android`).
pub trait PeerCallbacks: Send + Sync + 'static {
    fn receive_dir(&self) -> PathBuf;

    fn on_clipboard_push<'a>(
        &'a self,
        peer: &'a TrustedPeer,
        msg: ClipboardMessage,
        payload: ValidatedPayload,
    ) -> BoxFuture<'a, Result<ClipboardPushResponse, ApiErrorBody>>;

    fn on_clipboard_pull<'a>(
        &'a self,
        peer: &'a TrustedPeer,
    ) -> BoxFuture<'a, Result<ClipboardMessage, ApiErrorBody>>;

    #[allow(clippy::too_many_arguments)]
    fn on_file_receiving<'a>(
        &'a self,
        peer: &'a TrustedPeer,
        batch_id: &'a str,
        file_name: &'a str,
        file_index: usize,
        file_total: usize,
        bytes_received: u64,
        total_bytes: u64,
    ) -> BoxFuture<'a, ()>;

    fn on_file_received<'a>(
        &'a self,
        peer: &'a TrustedPeer,
        batch_id: &'a str,
        file_index: usize,
        file_total: usize,
        received: ReceivedFile,
    ) -> BoxFuture<'a, ()>;

    fn on_pair_requested<'a>(&'a self, view: PendingPairView) -> BoxFuture<'a, ()>;

    fn on_pair_completed<'a>(&'a self, peer: TrustedPeer) -> BoxFuture<'a, ()>;
}

#[derive(Debug, Clone)]
struct PairSession {
    session_id: String,
    peer_id: String,
    peer_name: String,
    peer_platform: Platform,
    peer_pubkey: [u8; 32],
    peer_cert_sha256_hex: String,
    peer_ip: String,
    peer_tls_port: u16,
    initiator_nonce: [u8; 32],
    responder_nonce: [u8; 32],
    sas_code: String,
    is_initiator: bool,
    local_approved: bool,
    remote_confirmed: bool,
    responder_confirm_sig_b64: Option<String>,
    state: PairState,
    expires_at_ms: u64,
}

impl PairSession {
    fn to_view(&self) -> PendingPairView {
        PendingPairView {
            session_id: self.session_id.clone(),
            peer_id: self.peer_id.clone(),
            peer_name: self.peer_name.clone(),
            peer_platform: self.peer_platform,
            peer_pubkey_b64: encode_pubkey_b64(&self.peer_pubkey),
            peer_fingerprint: format_pubkey_fingerprint(&self.peer_pubkey),
            peer_ip: self.peer_ip.clone(),
            peer_tls_port: self.peer_tls_port,
            sas_code: self.sas_code.clone(),
            is_initiator: self.is_initiator,
            state: self.state,
            expires_at_ms: self.expires_at_ms,
        }
    }

    fn transcript_ids<'a>(&'a self, local_id: &'a str) -> (&'a str, &'a str) {
        if self.is_initiator {
            (local_id, &self.peer_id)
        } else {
            (&self.peer_id, local_id)
        }
    }

    fn transcript_pks<'a>(&'a self, local_pk: &'a [u8; 32]) -> (&'a [u8; 32], &'a [u8; 32]) {
        if self.is_initiator {
            (local_pk, &self.peer_pubkey)
        } else {
            (&self.peer_pubkey, local_pk)
        }
    }
}

/// Manages active 6-digit SAS pairing sessions and mutual confirmation state.
#[derive(Clone)]
pub struct PairingManager {
    identity: Arc<DeviceIdentity>,
    trust: TrustRegistry,
    sessions: Arc<RwLock<HashMap<String, PairSession>>>,
}

impl PairingManager {
    pub fn new(identity: Arc<DeviceIdentity>, trust: TrustRegistry) -> Self {
        Self {
            identity,
            trust,
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn list_pending(&self) -> Vec<PendingPairView> {
        let now = now_timestamp_ms();
        let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
        Self::expire_locked(&mut guard, now);
        let mut views: Vec<PendingPairView> = guard
            .values()
            .filter(|s| matches!(s.state, PairState::Pending | PairState::ApprovedLocal))
            .map(PairSession::to_view)
            .collect();
        views.sort_by_key(|v| v.expires_at_ms);
        views
    }

    pub fn get_session(&self, session_id: &str) -> Option<PendingPairView> {
        let now = now_timestamp_ms();
        let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
        Self::expire_locked(&mut guard, now);
        guard.get(session_id).map(PairSession::to_view)
    }

    fn expire_locked(map: &mut HashMap<String, PairSession>, now: u64) {
        for sess in map.values_mut() {
            if matches!(sess.state, PairState::Pending | PairState::ApprovedLocal)
                && now > sess.expires_at_ms
            {
                sess.state = PairState::Expired;
            }
        }
        map.retain(|_, s| now.saturating_sub(s.expires_at_ms) < 60_000);
    }

    /// Initiate outgoing pairing from this device to `peer` over TLS, storing pending SAS session.
    pub async fn start_outgoing_pairing(
        &self,
        peer: &DiscoveredPeer,
    ) -> Result<PendingPairView, NetError> {
        let peer_pubkey = decode_pubkey_b64(&peer.pubkey_b64)?;
        if derive_device_id(&peer_pubkey) != peer.device_id {
            return Err(NetError::Protocol(
                "discovered peer device_id does not match Ed25519 pubkey".into(),
            ));
        }

        let session_id = Uuid::new_v4().to_string();
        let mut initiator_nonce = [0u8; 32];
        OsRng.fill_bytes(&mut initiator_nonce);

        let req_digest = pair_request_transcript(
            &session_id,
            &self.identity.device_id,
            &self.identity.pubkey_bytes(),
            &initiator_nonce,
        );
        let payload = PairRequestPayload {
            protocol: PROTOCOL_VERSION,
            session_id: session_id.clone(),
            initiator_id: self.identity.device_id.clone(),
            initiator_name: self.identity.device_name(),
            initiator_platform: self.identity.platform,
            initiator_pubkey_b64: self.identity.pubkey_b64(),
            initiator_tls_port: 0,
            initiator_nonce_b64: STANDARD.encode(initiator_nonce),
            signature_b64: self.identity.sign_b64(&req_digest),
        };

        let client = MtlsClient::new(self.identity.clone());
        let (resp, peer_cert_sha256_hex) = client
            .request_pair(&peer.ip, peer.tls_port, peer_pubkey, &peer.device_id, &payload)
            .await?;

        if !resp.ok || resp.session_id != session_id {
            return Err(NetError::Protocol("responder rejected pair request".into()));
        }
        let resp_pk = decode_pubkey_b64(&resp.responder_pubkey_b64)?;
        if resp_pk != peer_pubkey || derive_device_id(&resp_pk) != resp.responder_id {
            return Err(NetError::Protocol(
                "responder identity mismatch in PairRequestResponse".into(),
            ));
        }
        let resp_nonce_vec = STANDARD
            .decode(resp.responder_nonce_b64.trim())
            .map_err(|e| NetError::Protocol(format!("invalid responder nonce base64: {e}")))?;
        let responder_nonce: [u8; 32] = resp_nonce_vec
            .try_into()
            .map_err(|_| NetError::Protocol("responder nonce must be 32 bytes".into()))?;
        let resp_sig = STANDARD
            .decode(resp.signature_b64.trim())
            .map_err(|e| NetError::Protocol(format!("invalid responder signature base64: {e}")))?;

        let my_pk = self.identity.pubkey_bytes();
        let resp_digest = pair_response_transcript(
            &session_id,
            &self.identity.device_id,
            &resp.responder_id,
            &my_pk,
            &resp_pk,
            &initiator_nonce,
            &responder_nonce,
        );
        verify_ed25519_signature(&resp_pk, &resp_digest, &resp_sig)?;

        let expected_sas = compute_sas_code(&my_pk, &resp_pk, &initiator_nonce, &responder_nonce);
        if resp.sas_code != expected_sas {
            return Err(NetError::Protocol("SAS code mismatch during handshake".into()));
        }

        let session = PairSession {
            session_id: session_id.clone(),
            peer_id: resp.responder_id,
            peer_name: resp.responder_name,
            peer_platform: resp.responder_platform,
            peer_pubkey: resp_pk,
            peer_cert_sha256_hex,
            peer_ip: peer.ip.clone(),
            peer_tls_port: peer.tls_port,
            initiator_nonce,
            responder_nonce,
            sas_code: expected_sas,
            is_initiator: true,
            local_approved: false,
            remote_confirmed: false,
            responder_confirm_sig_b64: None,
            state: PairState::Pending,
            expires_at_ms: now_timestamp_ms() + PAIR_SESSION_TTL_MS,
        };
        let view = session.to_view();
        let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
        Self::expire_locked(&mut guard, now_timestamp_ms());
        guard.insert(session_id, session);
        Ok(view)
    }

    /// Initiator local user confirms the 6-digit SAS code matches: notify responder and check status.
    pub async fn approve_local(
        &self,
        session_id: &str,
        callbacks: Option<&dyn PeerCallbacks>,
    ) -> Result<PendingPairView, NetError> {
        enum Action {
            InitiatorSend {
                ip: String,
                port: u16,
                peer_pk: [u8; 32],
                peer_id: String,
                payload: PairConfirmPayload,
            },
            ResponderFinalize(TrustedPeer, PendingPairView),
            ResponderWait(PendingPairView),
        }

        let action = {
            let now = now_timestamp_ms();
            let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
            Self::expire_locked(&mut guard, now);
            let sess = guard
                .get_mut(session_id)
                .ok_or_else(|| NetError::Protocol("pairing session not found".into()))?;
            if matches!(sess.state, PairState::Rejected | PairState::Expired) {
                return Err(NetError::Protocol(format!(
                    "pairing session is already {:?}",
                    sess.state
                )));
            }

            sess.local_approved = true;
            if sess.is_initiator {
                sess.state = PairState::ApprovedLocal;
                let my_pk = self.identity.pubkey_bytes();
                let (init_id, resp_id) = sess.transcript_ids(&self.identity.device_id);
                let (init_pk, resp_pk) = sess.transcript_pks(&my_pk);
                let digest = pair_confirm_transcript(
                    &sess.session_id,
                    init_id,
                    resp_id,
                    init_pk,
                    resp_pk,
                    &sess.initiator_nonce,
                    &sess.responder_nonce,
                    b"initiator",
                );
                let payload = PairConfirmPayload {
                    session_id: sess.session_id.clone(),
                    initiator_id: self.identity.device_id.clone(),
                    initiator_pubkey_b64: self.identity.pubkey_b64(),
                    confirm_signature_b64: self.identity.sign_b64(&digest),
                };
                Action::InitiatorSend {
                    ip: sess.peer_ip.clone(),
                    port: sess.peer_tls_port,
                    peer_pk: sess.peer_pubkey,
                    peer_id: sess.peer_id.clone(),
                    payload,
                }
            } else {
                // Responder role: prepare responder signature.
                let my_pk = self.identity.pubkey_bytes();
                let (init_id, resp_id) = sess.transcript_ids(&self.identity.device_id);
                let (init_pk, resp_pk) = sess.transcript_pks(&my_pk);
                let digest = pair_confirm_transcript(
                    &sess.session_id,
                    init_id,
                    resp_id,
                    init_pk,
                    resp_pk,
                    &sess.initiator_nonce,
                    &sess.responder_nonce,
                    b"responder",
                );
                sess.responder_confirm_sig_b64 = Some(self.identity.sign_b64(&digest));

                if sess.remote_confirmed {
                    sess.state = PairState::Completed;
                    let trusted = TrustedPeer {
                        device_id: sess.peer_id.clone(),
                        device_name: sess.peer_name.clone(),
                        platform: sess.peer_platform,
                        pubkey_b64: encode_pubkey_b64(&sess.peer_pubkey),
                        cert_sha256_hex: sess.peer_cert_sha256_hex.clone(),
                        paired_at_ms: now,
                        last_known_ip: Some(sess.peer_ip.clone()),
                        tls_port: sess.peer_tls_port,
                    };
                    Action::ResponderFinalize(trusted, sess.to_view())
                } else {
                    sess.state = PairState::ApprovedLocal;
                    Action::ResponderWait(sess.to_view())
                }
            }
        };

        match action {
            Action::ResponderWait(view) => Ok(view),
            Action::ResponderFinalize(trusted, view) => {
                self.trust.upsert(trusted.clone())?;
                if let Some(cb) = callbacks {
                    cb.on_pair_completed(trusted).await;
                }
                Ok(view)
            }
            Action::InitiatorSend {
                ip,
                port,
                peer_pk,
                peer_id,
                payload,
            } => {
                let client = MtlsClient::new(self.identity.clone());
                let status = client
                    .confirm_pair(&ip, port, peer_pk, &peer_id, &payload)
                    .await?;
                self.apply_remote_status(session_id, &status, callbacks)
                    .await
            }
        }
    }

    /// Initiator polls responder status and finalizes trust ONLY after responder confirms & signs.
    pub async fn poll_outgoing_pair(
        &self,
        session_id: &str,
        callbacks: Option<&dyn PeerCallbacks>,
    ) -> Result<PendingPairView, NetError> {
        let (ip, port, peer_pk, peer_id, current_view) = {
            let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
            Self::expire_locked(&mut guard, now_timestamp_ms());
            let sess = guard
                .get(session_id)
                .ok_or_else(|| NetError::Protocol("pairing session not found".into()))?;
            if !sess.is_initiator {
                return Ok(sess.to_view());
            }
            if matches!(
                sess.state,
                PairState::Completed | PairState::Rejected | PairState::Expired
            ) {
                return Ok(sess.to_view());
            }
            (
                sess.peer_ip.clone(),
                sess.peer_tls_port,
                sess.peer_pubkey,
                sess.peer_id.clone(),
                sess.to_view(),
            )
        };

        let _ = current_view;
        let client = MtlsClient::new(self.identity.clone());
        let status = client
            .poll_pair_status(&ip, port, peer_pk, &peer_id, session_id)
            .await?;
        self.apply_remote_status(session_id, &status, callbacks)
            .await
    }

    async fn apply_remote_status(
        &self,
        session_id: &str,
        status: &PairStatusResponse,
        callbacks: Option<&dyn PeerCallbacks>,
    ) -> Result<PendingPairView, NetError> {
        enum Finish {
            None(PendingPairView),
            Persist(TrustedPeer, PendingPairView),
        }

        let finish = {
            let now = now_timestamp_ms();
            let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
            let sess = guard
                .get_mut(session_id)
                .ok_or_else(|| NetError::Protocol("pairing session not found".into()))?;

            match status.state {
                PairState::Rejected => {
                    sess.state = PairState::Rejected;
                    Finish::None(sess.to_view())
                }
                PairState::Expired => {
                    sess.state = PairState::Expired;
                    Finish::None(sess.to_view())
                }
                PairState::Completed => {
                    if !sess.local_approved {
                        // Never trust peer unless local user also approved!
                        return Ok(sess.to_view());
                    }
                    let sig_b64 = status
                        .responder_confirm_signature_b64
                        .as_deref()
                        .ok_or_else(|| {
                            NetError::Protocol(
                                "missing responder confirmation signature on completed pairing"
                                    .into(),
                            )
                        })?;
                    let sig_bytes = STANDARD.decode(sig_b64.trim()).map_err(|e| {
                        NetError::Protocol(format!("invalid responder confirm sig base64: {e}"))
                    })?;
                    let my_pk = self.identity.pubkey_bytes();
                    let (init_id, resp_id) = sess.transcript_ids(&self.identity.device_id);
                    let (init_pk, resp_pk) = sess.transcript_pks(&my_pk);
                    let digest = pair_confirm_transcript(
                        &sess.session_id,
                        init_id,
                        resp_id,
                        init_pk,
                        resp_pk,
                        &sess.initiator_nonce,
                        &sess.responder_nonce,
                        b"responder",
                    );
                    verify_ed25519_signature(&sess.peer_pubkey, &digest, &sig_bytes)?;

                    sess.remote_confirmed = true;
                    sess.state = PairState::Completed;
                    let trusted = TrustedPeer {
                        device_id: sess.peer_id.clone(),
                        device_name: sess.peer_name.clone(),
                        platform: sess.peer_platform,
                        pubkey_b64: encode_pubkey_b64(&sess.peer_pubkey),
                        cert_sha256_hex: sess.peer_cert_sha256_hex.clone(),
                        paired_at_ms: now,
                        last_known_ip: Some(sess.peer_ip.clone()),
                        tls_port: sess.peer_tls_port,
                    };
                    Finish::Persist(trusted, sess.to_view())
                }
                PairState::Pending | PairState::ApprovedLocal => Finish::None(sess.to_view()),
            }
        };

        match finish {
            Finish::None(view) => Ok(view),
            Finish::Persist(trusted, view) => {
                self.trust.upsert(trusted.clone())?;
                if let Some(cb) = callbacks {
                    cb.on_pair_completed(trusted).await;
                }
                Ok(view)
            }
        }
    }

    pub async fn reject_local(&self, session_id: &str) -> Result<PendingPairView, NetError> {
        let (view, maybe_remote) = {
            let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
            let sess = guard
                .get_mut(session_id)
                .ok_or_else(|| NetError::Protocol("pairing session not found".into()))?;
            sess.state = PairState::Rejected;
            let remote = if sess.is_initiator {
                Some((
                    sess.peer_ip.clone(),
                    sess.peer_tls_port,
                    sess.peer_pubkey,
                    sess.peer_id.clone(),
                ))
            } else {
                None
            };
            (sess.to_view(), remote)
        };

        if let Some((ip, port, pk, peer_id)) = maybe_remote {
            let client = MtlsClient::new(self.identity.clone());
            let _ = client.reject_pair(&ip, port, pk, &peer_id, session_id).await;
        }
        Ok(view)
    }

    fn handle_incoming_pair_request(
        &self,
        client: &ClientCertInfo,
        remote_ip: &str,
        req: PairRequestPayload,
    ) -> Result<(PairRequestResponse, PendingPairView), ApiErrorBody> {
        if req.protocol != PROTOCOL_VERSION {
            return Err(ApiErrorBody::new(
                ErrorCode::UnsupportedProtocol,
                "unsupported protocol version",
            ));
        }
        if req.initiator_id == self.identity.device_id {
            return Err(ApiErrorBody::new(
                ErrorCode::PairingFailed,
                "cannot pair device with itself",
            ));
        }
        let req_pk = decode_pubkey_b64(&req.initiator_pubkey_b64).map_err(|e| {
            ApiErrorBody::new(ErrorCode::PairingFailed, format!("invalid pubkey: {e}"))
        })?;
        if req_pk != client.pubkey || derive_device_id(&client.pubkey) != req.initiator_id {
            return Err(ApiErrorBody::new(
                ErrorCode::Unauthorized,
                "TLS client certificate does not match pair request identity",
            ));
        }

        let init_nonce_vec = STANDARD.decode(req.initiator_nonce_b64.trim()).map_err(|_| {
            ApiErrorBody::new(ErrorCode::PairingFailed, "invalid initiator nonce encoding")
        })?;
        let initiator_nonce: [u8; 32] = init_nonce_vec.try_into().map_err(|_| {
            ApiErrorBody::new(ErrorCode::PairingFailed, "initiator nonce must be 32 bytes")
        })?;
        let sig_bytes = STANDARD.decode(req.signature_b64.trim()).map_err(|_| {
            ApiErrorBody::new(ErrorCode::PairingFailed, "invalid request signature")
        })?;

        let req_digest = pair_request_transcript(
            &req.session_id,
            &req.initiator_id,
            &req_pk,
            &initiator_nonce,
        );
        verify_ed25519_signature(&req_pk, &req_digest, &sig_bytes).map_err(|e| {
            ApiErrorBody::new(
                ErrorCode::PairingFailed,
                format!("invalid request signature: {e}"),
            )
        })?;

        let mut responder_nonce = [0u8; 32];
        OsRng.fill_bytes(&mut responder_nonce);
        let my_pk = self.identity.pubkey_bytes();
        let sas_code = compute_sas_code(&req_pk, &my_pk, &initiator_nonce, &responder_nonce);
        let resp_digest = pair_response_transcript(
            &req.session_id,
            &req.initiator_id,
            &self.identity.device_id,
            &req_pk,
            &my_pk,
            &initiator_nonce,
            &responder_nonce,
        );

        let session = PairSession {
            session_id: req.session_id.clone(),
            peer_id: req.initiator_id.clone(),
            peer_name: req.initiator_name.clone(),
            peer_platform: req.initiator_platform,
            peer_pubkey: req_pk,
            peer_cert_sha256_hex: client.cert_sha256_hex.clone(),
            peer_ip: remote_ip.to_string(),
            peer_tls_port: if req.initiator_tls_port != 0 {
                req.initiator_tls_port
            } else {
                crate::limits::DEFAULT_TLS_PORT
            },
            initiator_nonce,
            responder_nonce,
            sas_code: sas_code.clone(),
            is_initiator: false,
            local_approved: false,
            remote_confirmed: false,
            responder_confirm_sig_b64: None,
            state: PairState::Pending,
            expires_at_ms: now_timestamp_ms() + PAIR_SESSION_TTL_MS,
        };
        let view = session.to_view();

        {
            let now = now_timestamp_ms();
            let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
            Self::expire_locked(&mut guard, now);
            let active_count = guard
                .values()
                .filter(|s| matches!(s.state, PairState::Pending | PairState::ApprovedLocal))
                .count();
            if active_count >= MAX_ACTIVE_PAIR_SESSIONS && !guard.contains_key(&req.session_id) {
                return Err(ApiErrorBody::new(
                    ErrorCode::PairingFailed,
                    "too many pending pairing sessions",
                ));
            }
            guard.insert(req.session_id.clone(), session);
        }

        let resp = PairRequestResponse {
            ok: true,
            session_id: req.session_id,
            responder_id: self.identity.device_id.clone(),
            responder_name: self.identity.device_name(),
            responder_platform: self.identity.platform,
            responder_pubkey_b64: self.identity.pubkey_b64(),
            responder_nonce_b64: STANDARD.encode(responder_nonce),
            sas_code,
            signature_b64: self.identity.sign_b64(&resp_digest),
        };
        Ok((resp, view))
    }

    fn handle_incoming_pair_confirm(
        &self,
        client: &ClientCertInfo,
        req: &PairConfirmPayload,
    ) -> Result<(PairStatusResponse, Option<TrustedPeer>), ApiErrorBody> {
        let now = now_timestamp_ms();
        let mut guard = self.sessions.write().expect("PairingManager lock poisoned");
        Self::expire_locked(&mut guard, now);

        let sess = guard.get_mut(&req.session_id).ok_or_else(|| {
            ApiErrorBody::new(ErrorCode::PairingNotFound, "pairing session not found")
        })?;
        if sess.is_initiator || sess.peer_pubkey != client.pubkey {
            return Err(ApiErrorBody::new(
                ErrorCode::Unauthorized,
                "pairing session peer identity mismatch",
            ));
        }
        if matches!(sess.state, PairState::Rejected | PairState::Expired) {
            return Err(ApiErrorBody::new(
                ErrorCode::PairingRejected,
                "pairing session rejected or expired",
            ));
        }

        let sig_bytes = STANDARD
            .decode(req.confirm_signature_b64.trim())
            .map_err(|_| ApiErrorBody::new(ErrorCode::PairingFailed, "invalid confirm sig"))?;
        let my_pk = self.identity.pubkey_bytes();
        let (init_id, resp_id) = sess.transcript_ids(&self.identity.device_id);
        let (init_pk, resp_pk) = sess.transcript_pks(&my_pk);
        let digest = pair_confirm_transcript(
            &sess.session_id,
            init_id,
            resp_id,
            init_pk,
            resp_pk,
            &sess.initiator_nonce,
            &sess.responder_nonce,
            b"initiator",
        );
        verify_ed25519_signature(&sess.peer_pubkey, &digest, &sig_bytes).map_err(|_| {
            ApiErrorBody::new(
                ErrorCode::PairingFailed,
                "invalid initiator confirmation signature",
            )
        })?;

        sess.remote_confirmed = true;
        let maybe_trusted = if sess.local_approved {
            sess.state = PairState::Completed;
            Some(TrustedPeer {
                device_id: sess.peer_id.clone(),
                device_name: sess.peer_name.clone(),
                platform: sess.peer_platform,
                pubkey_b64: encode_pubkey_b64(&sess.peer_pubkey),
                cert_sha256_hex: sess.peer_cert_sha256_hex.clone(),
                paired_at_ms: now,
                last_known_ip: Some(sess.peer_ip.clone()),
                tls_port: sess.peer_tls_port,
            })
        } else {
            None
        };

        let status = PairStatusResponse {
            ok: true,
            session_id: sess.session_id.clone(),
            state: sess.state,
            responder_confirm_signature_b64: if sess.state == PairState::Completed {
                sess.responder_confirm_sig_b64.clone()
            } else {
                None
            },
        };
        Ok((status, maybe_trusted))
    }
}

#[derive(Clone)]
struct ServerAppState {
    identity: Arc<DeviceIdentity>,
    trust: TrustRegistry,
    pairing: PairingManager,
    callbacks: Arc<dyn PeerCallbacks>,
}

#[derive(Clone)]
struct ConnectionContext {
    client: ClientCertInfo,
    remote_ip: String,
}

#[derive(Clone)]
struct PeerScopedService {
    router: Router,
    conn_ctx: ConnectionContext,
}

impl hyper::service::Service<Request<hyper::body::Incoming>> for PeerScopedService {
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, mut req: Request<hyper::body::Incoming>) -> Self::Future {
        req.extensions_mut().insert(self.conn_ctx.clone());
        let router = self.router.clone();
        Box::pin(async move {
            let axum_req = req.map(Body::new);
            router.oneshot(axum_req).await
        })
    }
}

/// Handle to a running mTLS 1.3 HTTPS server.
pub struct MtlsServerHandle {
    local_addr: SocketAddr,
    join: tokio::task::JoinHandle<()>,
}

impl MtlsServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn wait(self) {
        let _ = self.join.await;
    }
}

/// Start the mTLS 1.3 HTTPS server enforcing client certificate pinning and `TrustRegistry`.
pub async fn start_mtls_server(
    bind_addr: SocketAddr,
    identity: Arc<DeviceIdentity>,
    trust: TrustRegistry,
    pairing: PairingManager,
    callbacks: Arc<dyn PeerCallbacks>,
    auth_policy: TlsAuthPolicy,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<MtlsServerHandle, NetError> {
    let tls_config = match auth_policy {
        TlsAuthPolicy::AllowPairingRequests => build_mtls_server_config(&identity)?,
        TlsAuthPolicy::TrustedOnly => {
            build_mtls_server_config_with_filter(&identity, Some(trust.trust_predicate()))?
        }
    };
    let acceptor = TlsAcceptor::from(tls_config);
    let listener = TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;

    let app_state = ServerAppState {
        identity: identity.clone(),
        trust: trust.clone(),
        pairing,
        callbacks,
    };

    let router = Router::new()
        .route("/v1/heartbeat", post(handle_heartbeat))
        .route("/v1/clipboard", post(handle_clipboard_push).get(handle_clipboard_pull))
        .route("/v1/files/upload", post(handle_file_upload))
        .route("/v1/pair/request", post(handle_pair_request))
        .route("/v1/pair/confirm", post(handle_pair_confirm))
        .route("/v1/pair/status/:session_id", get(handle_pair_status))
        .route("/v1/pair/reject", post(handle_pair_reject))
        .with_state(app_state);

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                accept_res = listener.accept() => {
                    let (tcp, peer_addr) = match accept_res {
                        Ok(pair) => pair,
                        Err(err) => {
                            warn!("TCP accept error: {err}");
                            continue;
                        }
                    };
                    let _ = tcp.set_nodelay(true);
                    let acceptor = acceptor.clone();
                    let router = router.clone();
                    let conn_shutdown = shutdown_rx.clone();

                    tokio::spawn(async move {
                        let tls_stream = match acceptor.accept(tcp).await {
                            Ok(s) => s,
                            Err(err) => {
                                debug!("mTLS handshake rejected from {peer_addr}: {err}");
                                return;
                            }
                        };
                        let client = {
                            let (_, session) = tls_stream.get_ref();
                            let Some(certs) = session.peer_certificates() else {
                                return;
                            };
                            let Some(first) = certs.first() else {
                                return;
                            };
                            let Ok(pubkey) = extract_ed25519_pubkey_from_cert(first.as_ref()) else {
                                return;
                            };
                            ClientCertInfo {
                                pubkey,
                                pubkey_b64: encode_pubkey_b64(&pubkey),
                                device_id: derive_device_id(&pubkey),
                                cert_sha256_hex: sha256_hex(first.as_ref()),
                            }
                        };

                        let service = PeerScopedService {
                            router,
                            conn_ctx: ConnectionContext {
                                client,
                                remote_ip: peer_addr.ip().to_string(),
                            },
                        };

                        let io = TokioIo::new(tls_stream);
                        let conn = server_http1::Builder::new().serve_connection(io, service);
                        tokio::pin!(conn);
                        let mut shutdown_watch = conn_shutdown;
                        tokio::select! {
                            res = &mut conn => {
                                if let Err(err) = res {
                                    debug!("HTTP connection ended ({peer_addr}): {err}");
                                }
                            }
                            _ = shutdown_watch.changed() => {
                                if *shutdown_watch.borrow() {
                                    conn.as_mut().graceful_shutdown();
                                }
                            }
                        }
                    });
                }
            }
        }
    });

    Ok(MtlsServerHandle { local_addr, join })
}

fn require_trusted_peer(
    trust: &TrustRegistry,
    conn_ctx: &ConnectionContext,
) -> Result<TrustedPeer, Box<Response>> {
    let Some(peer) = trust.get_by_pubkey(&conn_ctx.client.pubkey) else {
        return Err(Box::new(error_response(
            StatusCode::FORBIDDEN,
            ErrorCode::UntrustedPeer,
            "client device is not paired or trust was revoked",
        )));
    };
    if peer.device_id != conn_ctx.client.device_id {
        return Err(Box::new(error_response(
            StatusCode::FORBIDDEN,
            ErrorCode::UntrustedPeer,
            "client device identifier mismatch",
        )));
    }
    Ok(peer)
}

fn error_response(status: StatusCode, code: ErrorCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiErrorBody::new(code, msg))).into_response()
}

fn api_error_status(err: &ApiErrorBody) -> StatusCode {
    match err.code {
        ErrorCode::Unauthorized | ErrorCode::UntrustedPeer | ErrorCode::PairingRejected => {
            StatusCode::FORBIDDEN
        }
        ErrorCode::PairingNotFound | ErrorCode::ImageNotFound | ErrorCode::ClipboardEmpty => {
            StatusCode::NOT_FOUND
        }
        ErrorCode::PayloadTooLarge | ErrorCode::ImageTooLarge | ErrorCode::TextTooLarge => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ErrorCode::SyncPaused => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::InternalError | ErrorCode::TransferFailed => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    }
}

async fn handle_heartbeat(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
    Json(req): Json<HeartbeatRequest>,
) -> Response {
    let peer = match require_trusted_peer(&state.trust, &conn_ctx) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    if req.protocol != PROTOCOL_VERSION {
        return error_response(
            StatusCode::BAD_REQUEST,
            ErrorCode::UnsupportedProtocol,
            "unsupported protocol version",
        );
    }
    if req.device_id != peer.device_id {
        return error_response(
            StatusCode::FORBIDDEN,
            ErrorCode::Unauthorized,
            "heartbeat device_id does not match client certificate",
        );
    }
    let _ = state.trust.update_seen_endpoint(
        &peer.device_id,
        Some(&conn_ctx.remote_ip),
        req.tls_port,
        Some(&req.device_name),
    );

    Json(HeartbeatResponse {
        ok: true,
        protocol: PROTOCOL_VERSION,
        device_id: state.identity.device_id.clone(),
        device_name: state.identity.device_name(),
        platform: state.identity.platform,
        tls_port: req.tls_port,
        timestamp_ms: now_timestamp_ms(),
    })
    .into_response()
}

async fn handle_clipboard_push(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
    body: Body,
) -> Response {
    let peer = match require_trusted_peer(&state.trust, &conn_ctx) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let bytes = match axum::body::to_bytes(body, MAX_HTTP_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::PayloadTooLarge,
                "clipboard request exceeds maximum size",
            );
        }
    };
    let msg: ClipboardMessage = match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(err) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidMessage,
                format!("invalid clipboard JSON: {err}"),
            );
        }
    };
    let validated = match msg.validate_and_decode() {
        Ok(v) => v,
        Err(err) => {
            return error_response(StatusCode::BAD_REQUEST, err.code(), err.to_string());
        }
    };

    match state.callbacks.on_clipboard_push(&peer, msg, validated).await {
        Ok(ack) => (StatusCode::OK, Json(ack)).into_response(),
        Err(err) => (api_error_status(&err), Json(err)).into_response(),
    }
}

async fn handle_clipboard_pull(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
) -> Response {
    let peer = match require_trusted_peer(&state.trust, &conn_ctx) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };

    match state.callbacks.on_clipboard_pull(&peer).await {
        Ok(msg) => (StatusCode::OK, Json(msg)).into_response(),
        Err(err) => (api_error_status(&err), Json(err)).into_response(),
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

async fn handle_file_upload(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let peer = match require_trusted_peer(&state.trust, &conn_ctx) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };

    let batch_id = header_str(&headers, HDR_BATCH_ID).unwrap_or_else(|| Uuid::new_v4().to_string());
    let raw_name_enc = match header_str(&headers, HDR_FILE_NAME) {
        Some(v) => v,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidMessage,
                "missing X-Workspace-File-Name header",
            );
        }
    };
    let requested_name = match percent_decode_utf8(&raw_name_enc) {
        Ok(name) => name,
        Err(err) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::InvalidMessage, err.to_string());
        }
    };
    let declared_size = header_str(&headers, HDR_FILE_SIZE)
        .and_then(|v| v.parse::<u64>().ok());
    let file_index = header_str(&headers, HDR_FILE_INDEX)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let file_total = header_str(&headers, HDR_FILE_TOTAL)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let expected_blake3 = header_str(&headers, HDR_FILE_BLAKE3);

    let dest_dir = state.callbacks.receive_dir();
    let mut receiver = match FileReceiver::open(
        &dest_dir,
        &requested_name,
        declared_size,
        expected_blake3,
    )
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::TransferFailed,
                format!("failed to stage incoming file: {err}"),
            );
        }
    };

    let total_for_progress = declared_size.unwrap_or(0);
    let mut last_progress = Instant::now();
    let mut stream = body.into_data_stream();

    while let Some(next_chunk) = stream.next().await {
        let chunk = match next_chunk {
            Ok(bytes) => bytes,
            Err(err) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::TransferFailed,
                    format!("upload stream interrupted: {err}"),
                );
            }
        };
        if let Err(err) = receiver.write_chunk(&chunk).await {
            return error_response(
                StatusCode::BAD_REQUEST,
                ErrorCode::TransferFailed,
                err.to_string(),
            );
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            state
                .callbacks
                .on_file_receiving(
                    &peer,
                    &batch_id,
                    &requested_name,
                    file_index,
                    file_total,
                    receiver.bytes_written(),
                    total_for_progress,
                )
                .await;
        }
    }

    let received = match receiver.finish().await {
        Ok(done) => done,
        Err(err) => {
            let code = match &err {
                crate::transfer::TransferError::ChecksumMismatch => ErrorCode::ChecksumMismatch,
                crate::transfer::TransferError::SizeMismatch { .. } => ErrorCode::SizeMismatch,
                _ => ErrorCode::TransferFailed,
            };
            return error_response(StatusCode::BAD_REQUEST, code, err.to_string());
        }
    };

    let resp = FileUploadResponse {
        ok: true,
        batch_id: batch_id.clone(),
        file_name: received.file_name.clone(),
        saved_path: received.path.display().to_string(),
        size: received.size,
        checksum: received.blake3_hex.clone(),
    };

    state
        .callbacks
        .on_file_received(&peer, &batch_id, file_index, file_total, received)
        .await;

    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_pair_request(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
    Json(req): Json<PairRequestPayload>,
) -> Response {
    match state
        .pairing
        .handle_incoming_pair_request(&conn_ctx.client, &conn_ctx.remote_ip, req)
    {
        Ok((resp, view)) => {
            state.callbacks.on_pair_requested(view).await;
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(err) => (api_error_status(&err), Json(err)).into_response(),
    }
}

async fn handle_pair_confirm(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
    Json(req): Json<PairConfirmPayload>,
) -> Response {
    match state.pairing.handle_incoming_pair_confirm(&conn_ctx.client, &req) {
        Ok((status, maybe_trusted)) => {
            if let Some(trusted) = maybe_trusted {
                if let Err(err) = state.trust.upsert(trusted.clone()) {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        ErrorCode::InternalError,
                        format!("failed to persist paired device: {err}"),
                    );
                }
                state.callbacks.on_pair_completed(trusted).await;
            }
            (StatusCode::OK, Json(status)).into_response()
        }
        Err(err) => (api_error_status(&err), Json(err)).into_response(),
    }
}

async fn handle_pair_status(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    let now = now_timestamp_ms();
    let mut guard = state
        .pairing
        .sessions
        .write()
        .expect("PairingManager lock poisoned");
    PairingManager::expire_locked(&mut guard, now);

    let Some(sess) = guard.get(&session_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::PairingNotFound,
            "pairing session not found",
        );
    };
    if sess.peer_pubkey != conn_ctx.client.pubkey {
        return error_response(
            StatusCode::FORBIDDEN,
            ErrorCode::Unauthorized,
            "pairing status caller identity mismatch",
        );
    }

    (
        StatusCode::OK,
        Json(PairStatusResponse {
            ok: true,
            session_id: sess.session_id.clone(),
            state: sess.state,
            responder_confirm_signature_b64: if sess.state == PairState::Completed {
                sess.responder_confirm_sig_b64.clone()
            } else {
                None
            },
        }),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct RejectSessionReq {
    session_id: String,
}

async fn handle_pair_reject(
    State(state): State<ServerAppState>,
    Extension(conn_ctx): Extension<ConnectionContext>,
    Json(req): Json<RejectSessionReq>,
) -> Response {
    let mut guard = state
        .pairing
        .sessions
        .write()
        .expect("PairingManager lock poisoned");
    let Some(sess) = guard.get_mut(&req.session_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::PairingNotFound,
            "pairing session not found",
        );
    };
    if sess.peer_pubkey != conn_ctx.client.pubkey {
        return error_response(
            StatusCode::FORBIDDEN,
            ErrorCode::Unauthorized,
            "pairing reject caller identity mismatch",
        );
    }
    sess.state = PairState::Rejected;
    (
        StatusCode::OK,
        Json(PairStatusResponse {
            ok: true,
            session_id: sess.session_id.clone(),
            state: PairState::Rejected,
            responder_confirm_signature_b64: None,
        }),
    )
        .into_response()
}

type ClientBody = UnsyncBoxBody<Bytes, std::io::Error>;

/// Progress callback invoked while streaming a file upload `(bytes_sent, total_bytes)`.
pub type UploadProgressFn = Arc<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct FileUploadMeta {
    pub batch_id: String,
    pub file_name: String,
    pub total_size: u64,
    pub file_index: usize,
    pub file_total: usize,
    pub blake3_hex: Option<String>,
}

async fn connect_tcp_preferring_lan(ip: &str, port: u16) -> std::io::Result<TcpStream> {
    if let Ok(target_ip) = ip.parse::<std::net::IpAddr>() {
        if let std::net::IpAddr::V4(target_v4) = target_ip
            && !target_v4.is_loopback()
            && let Some(iface) = crate::discovery::find_lan_interface_for_ipv4(target_v4)
            && let Ok(sock) = tokio::net::TcpSocket::new_v4()
        {
            #[cfg(any(target_os = "android", target_os = "linux"))]
            {
                if let Err(err) = sock.bind_device(Some(iface.name.as_bytes())) {
                    debug!("TCP SO_BINDTODEVICE on {} skipped: {err}", iface.name);
                }
            }
            let bind_addr = SocketAddr::V4(std::net::SocketAddrV4::new(iface.ip, 0));
            let remote = SocketAddr::new(std::net::IpAddr::V4(target_v4), port);
            if sock.bind(bind_addr).is_ok()
                && let Ok(stream) = sock.connect(remote).await
            {
                return Ok(stream);
            }
        }
        return TcpStream::connect(SocketAddr::new(target_ip, port)).await;
    }
    TcpStream::connect((ip, port)).await
}

/// Pinned mTLS 1.3 HTTPS client for communicating with a single verified peer.
#[derive(Clone)]
pub struct MtlsClient {
    identity: Arc<DeviceIdentity>,
}

impl MtlsClient {
    pub fn new(identity: Arc<DeviceIdentity>) -> Self {
        Self { identity }
    }

    async fn connect(
        &self,
        ip: &str,
        tls_port: u16,
        expected_server_pubkey: [u8; 32],
        peer_device_id: &str,
    ) -> Result<(http1::SendRequest<ClientBody>, tokio::task::JoinHandle<()>, String), NetError>
    {
        let client_cfg = build_mtls_client_config(&self.identity, expected_server_pubkey)?;
        let connector = TlsConnector::from(client_cfg);
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, connect_tcp_preferring_lan(ip, tls_port))
            .await
            .map_err(|_| NetError::Timeout)??;
        let _ = tcp.set_nodelay(true);

        let server_name = workspace_server_name(peer_device_id);
        let tls_stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp))
            .await
            .map_err(|_| NetError::Timeout)??;

        let peer_cert_sha256 = {
            let (_, session) = tls_stream.get_ref();
            session
                .peer_certificates()
                .and_then(|c| c.first())
                .map(|c| sha256_hex(c.as_ref()))
                .unwrap_or_default()
        };

        let (sender, conn) = http1::handshake(TokioIo::new(tls_stream)).await?;
        let driver = tokio::spawn(async move {
            if let Err(err) = conn.await {
                debug!("client mTLS HTTP connection ended: {err}");
            }
        });

        Ok((sender, driver, peer_cert_sha256))
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_json<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        ip: &str,
        tls_port: u16,
        expected_server_pubkey: [u8; 32],
        peer_device_id: &str,
        method: http::Method,
        path: &str,
        payload: Option<&Req>,
    ) -> Result<(Resp, String), NetError> {
        let (mut sender, driver, peer_cert_sha256) = self
            .connect(ip, tls_port, expected_server_pubkey, peer_device_id)
            .await?;

        let body_bytes = match payload {
            Some(p) => Bytes::from(serde_json::to_vec(p)?),
            None => Bytes::new(),
        };
        let len_str = body_bytes.len().to_string();
        let body: ClientBody = Full::new(body_bytes)
            .map_err(|never| match never {})
            .boxed_unsync();

        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(http::header::HOST, "workspace.local")
            .header(http::header::CONTENT_LENGTH, len_str);
        if payload.is_some() {
            builder = builder.header(http::header::CONTENT_TYPE, "application/json");
        }
        let req = builder
            .body(body)
            .map_err(|e| NetError::Protocol(e.to_string()))?;

        let resp = tokio::time::timeout(JSON_REQUEST_TIMEOUT, sender.send_request(req))
            .await
            .map_err(|_| NetError::Timeout)??;
        let status = resp.status();
        let collected = tokio::time::timeout(JSON_REQUEST_TIMEOUT, resp.into_body().collect())
            .await
            .map_err(|_| NetError::Timeout)??
            .to_bytes();

        drop(sender);
        let _ = driver.await;

        if !status.is_success() {
            return Err(NetError::RemoteStatus {
                status,
                body: String::from_utf8_lossy(&collected).to_string(),
            });
        }

        let parsed: Resp = serde_json::from_slice(&collected)?;
        Ok((parsed, peer_cert_sha256))
    }

    pub async fn request_pair(
        &self,
        ip: &str,
        tls_port: u16,
        expected_server_pubkey: [u8; 32],
        peer_device_id: &str,
        payload: &PairRequestPayload,
    ) -> Result<(PairRequestResponse, String), NetError> {
        self.send_json(
            ip,
            tls_port,
            expected_server_pubkey,
            peer_device_id,
            http::Method::POST,
            "/v1/pair/request",
            Some(payload),
        )
        .await
    }

    pub async fn confirm_pair(
        &self,
        ip: &str,
        tls_port: u16,
        expected_server_pubkey: [u8; 32],
        peer_device_id: &str,
        payload: &PairConfirmPayload,
    ) -> Result<PairStatusResponse, NetError> {
        let (resp, _) = self
            .send_json(
                ip,
                tls_port,
                expected_server_pubkey,
                peer_device_id,
                http::Method::POST,
                "/v1/pair/confirm",
                Some(payload),
            )
            .await?;
        Ok(resp)
    }

    pub async fn poll_pair_status(
        &self,
        ip: &str,
        tls_port: u16,
        expected_server_pubkey: [u8; 32],
        peer_device_id: &str,
        session_id: &str,
    ) -> Result<PairStatusResponse, NetError> {
        let path = format!("/v1/pair/status/{session_id}");
        let (resp, _) = self
            .send_json::<(), PairStatusResponse>(
                ip,
                tls_port,
                expected_server_pubkey,
                peer_device_id,
                http::Method::GET,
                &path,
                None,
            )
            .await?;
        Ok(resp)
    }

    pub async fn reject_pair(
        &self,
        ip: &str,
        tls_port: u16,
        expected_server_pubkey: [u8; 32],
        peer_device_id: &str,
        session_id: &str,
    ) -> Result<PairStatusResponse, NetError> {
        #[derive(Serialize)]
        struct Body<'a> {
            session_id: &'a str,
        }
        let (resp, _) = self
            .send_json(
                ip,
                tls_port,
                expected_server_pubkey,
                peer_device_id,
                http::Method::POST,
                "/v1/pair/reject",
                Some(&Body { session_id }),
            )
            .await?;
        Ok(resp)
    }

    pub async fn heartbeat(
        &self,
        peer: &TrustedPeer,
        ip: &str,
        my_tls_port: u16,
    ) -> Result<HeartbeatResponse, NetError> {
        let peer_pk = decode_pubkey_b64(&peer.pubkey_b64)?;
        let req = HeartbeatRequest {
            protocol: PROTOCOL_VERSION,
            device_id: self.identity.device_id.clone(),
            device_name: self.identity.device_name(),
            platform: self.identity.platform,
            tls_port: my_tls_port,
            timestamp_ms: now_timestamp_ms(),
        };
        let (resp, _) = self
            .send_json(
                ip,
                peer.tls_port,
                peer_pk,
                &peer.device_id,
                http::Method::POST,
                "/v1/heartbeat",
                Some(&req),
            )
            .await?;
        Ok(resp)
    }

    pub async fn push_clipboard(
        &self,
        peer: &TrustedPeer,
        ip: &str,
        msg: &ClipboardMessage,
    ) -> Result<ClipboardPushResponse, NetError> {
        let peer_pk = decode_pubkey_b64(&peer.pubkey_b64)?;
        let (resp, _) = self
            .send_json(
                ip,
                peer.tls_port,
                peer_pk,
                &peer.device_id,
                http::Method::POST,
                "/v1/clipboard",
                Some(msg),
            )
            .await?;
        Ok(resp)
    }

    pub async fn pull_clipboard(
        &self,
        peer: &TrustedPeer,
        ip: &str,
    ) -> Result<ClipboardMessage, NetError> {
        let peer_pk = decode_pubkey_b64(&peer.pubkey_b64)?;
        let (resp, _) = self
            .send_json::<(), ClipboardMessage>(
                ip,
                peer.tls_port,
                peer_pk,
                &peer.device_id,
                http::Method::GET,
                "/v1/clipboard",
                None,
            )
            .await?;
        Ok(resp)
    }

    /// Stream a local file cleanly to `peer` with incremental progress callbacks and cancellation.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_file_path(
        &self,
        peer: &TrustedPeer,
        ip: &str,
        path: &Path,
        batch_id: &str,
        file_index: usize,
        file_total: usize,
        on_progress: Option<UploadProgressFn>,
        cancel_rx: Option<watch::Receiver<bool>>,
    ) -> Result<FileUploadResponse, NetError> {
        let metadata = tokio::fs::metadata(path).await?;
        let total_size = metadata.len();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let blake3_hex = if total_size <= PRE_HASH_THRESHOLD_BYTES {
            let p = path.to_path_buf();
            Some(
                tokio::task::spawn_blocking(move || hash_file_blake3(&p))
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))??,
            )
        } else {
            None
        };

        let file = tokio::fs::File::open(path).await?;
        self.upload_reader(
            peer,
            ip,
            file,
            FileUploadMeta {
                batch_id: batch_id.to_string(),
                file_name,
                total_size,
                file_index,
                file_total,
                blake3_hex,
            },
            on_progress,
            cancel_rx,
        )
        .await
    }

    /// Stream an arbitrary `AsyncRead` payload cleanly over mTLS `POST /v1/files/upload`.
    pub async fn upload_reader<R: AsyncRead + Unpin + Send + 'static>(
        &self,
        peer: &TrustedPeer,
        ip: &str,
        reader: R,
        meta: FileUploadMeta,
        on_progress: Option<UploadProgressFn>,
        cancel_rx: Option<watch::Receiver<bool>>,
    ) -> Result<FileUploadResponse, NetError> {
        let peer_pk = decode_pubkey_b64(&peer.pubkey_b64)?;
        let (mut sender, driver, _) = self
            .connect(ip, peer.tls_port, peer_pk, &peer.device_id)
            .await?;

        let total_size = meta.total_size;
        let mut sent: u64 = 0;
        let mut last_tick = Instant::now();
        let cancel_rx_stream = cancel_rx.clone();
        let chunk_stream = ReaderStream::with_capacity(reader, STREAM_CHUNK_SIZE).map(move |item| {
            if let Some(ref rx) = cancel_rx_stream
                && *rx.borrow()
            {
                return Err(std::io::Error::new(
                    ErrorKind::Interrupted,
                    "transfer cancelled",
                ));
            }
            let bytes = item?;
            sent += bytes.len() as u64;
            if let Some(ref cb) = on_progress
                && (last_tick.elapsed() >= PROGRESS_INTERVAL || sent >= total_size)
            {
                last_tick = Instant::now();
                cb(sent, total_size);
            }
            Ok(Frame::data(bytes))
        });

        let body: ClientBody = BodyExt::boxed_unsync(StreamBody::new(chunk_stream));
        let encoded_name = percent_encode_utf8(&meta.file_name);

        let mut builder = Request::builder()
            .method(http::Method::POST)
            .uri("/v1/files/upload")
            .header(http::header::HOST, "workspace.local")
            .header(http::header::CONTENT_LENGTH, total_size.to_string())
            .header(HDR_BATCH_ID, meta.batch_id)
            .header(HDR_FILE_NAME, encoded_name)
            .header(HDR_FILE_SIZE, total_size.to_string())
            .header(HDR_FILE_INDEX, meta.file_index.to_string())
            .header(HDR_FILE_TOTAL, meta.file_total.to_string());

        if let Some(hash) = meta.blake3_hex
            && let Ok(hv) = HeaderValue::from_str(&hash)
        {
            builder = builder.header(HDR_FILE_BLAKE3, hv);
        }

        let req = builder
            .body(body)
            .map_err(|e| NetError::Protocol(e.to_string()))?;

        let send_fut = sender.send_request(req);
        let resp = if let Some(mut rx) = cancel_rx {
            if *rx.borrow() {
                return Err(NetError::Cancelled);
            }
            tokio::select! {
                res = send_fut => res.map_err(|err| {
                    if *rx.borrow() || err.to_string().contains("cancelled") {
                        NetError::Cancelled
                    } else {
                        NetError::Http(err)
                    }
                })?,
                _ = async {
                    while rx.changed().await.is_ok() {
                        if *rx.borrow() {
                            break;
                        }
                    }
                } => {
                    return Err(NetError::Cancelled);
                }
            }
        } else {
            send_fut.await.map_err(NetError::Http)?
        };
        let status = resp.status();
        let collected = resp.into_body().collect().await?.to_bytes();

        drop(sender);
        let _ = driver.await;

        if !status.is_success() {
            return Err(NetError::RemoteStatus {
                status,
                body: String::from_utf8_lossy(&collected).to_string(),
            });
        }
        Ok(serde_json::from_slice(&collected)?)
    }
}

fn hash_file_blake3(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; STREAM_CHUNK_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct TestCallbacks {
        recv_dir: PathBuf,
        latest_clip: Mutex<Option<ClipboardMessage>>,
        received_files: Mutex<Vec<ReceivedFile>>,
        pair_requests: Mutex<Vec<PendingPairView>>,
        completed_peers: Mutex<Vec<TrustedPeer>>,
    }

    impl TestCallbacks {
        fn new(recv_dir: PathBuf) -> Self {
            Self {
                recv_dir,
                latest_clip: Mutex::new(None),
                received_files: Mutex::new(Vec::new()),
                pair_requests: Mutex::new(Vec::new()),
                completed_peers: Mutex::new(Vec::new()),
            }
        }
    }

    impl PeerCallbacks for TestCallbacks {
        fn receive_dir(&self) -> PathBuf {
            self.recv_dir.clone()
        }

        fn on_clipboard_push<'a>(
            &'a self,
            _peer: &'a TrustedPeer,
            msg: ClipboardMessage,
            _payload: ValidatedPayload,
        ) -> BoxFuture<'a, Result<ClipboardPushResponse, ApiErrorBody>> {
            Box::pin(async move {
                let ack = ClipboardPushResponse {
                    ok: true,
                    applied: true,
                    deduplicated: false,
                    message_id: msg.message_id.clone(),
                    checksum: msg.checksum.clone(),
                };
                *self.latest_clip.lock().unwrap() = Some(msg);
                Ok(ack)
            })
        }

        fn on_clipboard_pull<'a>(
            &'a self,
            _peer: &'a TrustedPeer,
        ) -> BoxFuture<'a, Result<ClipboardMessage, ApiErrorBody>> {
            Box::pin(async move {
                self.latest_clip
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| ApiErrorBody::new(ErrorCode::ClipboardEmpty, "empty"))
            })
        }

        #[allow(clippy::too_many_arguments)]
    fn on_file_receiving<'a>(
            &'a self,
            _peer: &'a TrustedPeer,
            _batch_id: &'a str,
            _file_name: &'a str,
            _file_index: usize,
            _file_total: usize,
            _bytes_received: u64,
            _total_bytes: u64,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }

        fn on_file_received<'a>(
            &'a self,
            _peer: &'a TrustedPeer,
            _batch_id: &'a str,
            _file_index: usize,
            _file_total: usize,
            received: ReceivedFile,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.received_files.lock().unwrap().push(received);
            })
        }

        fn on_pair_requested<'a>(&'a self, view: PendingPairView) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.pair_requests.lock().unwrap().push(view);
            })
        }

        fn on_pair_completed<'a>(&'a self, peer: TrustedPeer) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.completed_peers.lock().unwrap().push(peer);
            })
        }
    }

    async fn spawn_node(
        name: &str,
        platform: Platform,
        policy: TlsAuthPolicy,
    ) -> (
        TempDir,
        Arc<DeviceIdentity>,
        TrustRegistry,
        PairingManager,
        Arc<TestCallbacks>,
        MtlsServerHandle,
        watch::Sender<bool>,
    ) {
        let dir = TempDir::new().unwrap();
        let id = Arc::new(DeviceIdentity::generate(name, platform).unwrap());
        let trust = TrustRegistry::load_in_dir(dir.path()).unwrap();
        let pairing = PairingManager::new(id.clone(), trust.clone());
        let cb = Arc::new(TestCallbacks::new(dir.path().join("downloads")));
        let (tx, rx) = watch::channel(false);
        let server = start_mtls_server(
            "127.0.0.1:0".parse().unwrap(),
            id.clone(),
            trust.clone(),
            pairing.clone(),
            cb.clone(),
            policy,
            rx,
        )
        .await
        .unwrap();
        (dir, id, trust, pairing, cb, server, tx)
    }

    fn as_discovered(id: &DeviceIdentity, addr: SocketAddr) -> DiscoveredPeer {
        DiscoveredPeer {
            device_id: id.device_id.clone(),
            device_name: id.device_name(),
            platform: id.platform,
            ip: "127.0.0.1".into(),
            tls_port: addr.port(),
            pubkey_b64: id.pubkey_b64(),
            fingerprint: id.fingerprint(),
            is_trusted: false,
            last_seen_ms: now_timestamp_ms(),
        }
    }

    fn as_trusted_target(id: &DeviceIdentity, addr: SocketAddr) -> TrustedPeer {
        TrustedPeer {
            device_id: id.device_id.clone(),
            device_name: id.device_name(),
            platform: id.platform,
            pubkey_b64: id.pubkey_b64(),
            cert_sha256_hex: id.cert_sha256_hex(),
            paired_at_ms: 0,
            last_known_ip: Some("127.0.0.1".into()),
            tls_port: addr.port(),
        }
    }

    #[tokio::test]
    async fn unpaired_and_revoked_peers_are_always_rejected() {
        let (_d_srv, id_srv, trust_srv, _p_srv, cb_srv, srv, tx_srv) =
            spawn_node("LinuxPC", Platform::Linux, TlsAuthPolicy::AllowPairingRequests).await;
        let (_d_rogue, id_rogue, _t_r, _p_r, _cb_r, srv_r, tx_r) =
            spawn_node("RoguePhone", Platform::Android, TlsAuthPolicy::AllowPairingRequests).await;
        let _ = srv_r;

        let rogue_client = MtlsClient::new(id_rogue.clone());
        let srv_target = as_trusted_target(&id_srv, srv.local_addr());

        // 1. Rogue heartbeat rejected with 403 UntrustedPeer
        let hb = rogue_client.heartbeat(&srv_target, "127.0.0.1", 45877).await;
        assert!(matches!(
            hb,
            Err(NetError::RemoteStatus {
                status: StatusCode::FORBIDDEN,
                ..
            })
        ));

        // 2. Rogue clipboard push rejected with 403
        let msg = ClipboardMessage::new_text(&id_rogue.device_id, 1, "secret").unwrap();
        let cp = rogue_client.push_clipboard(&srv_target, "127.0.0.1", &msg).await;
        assert!(matches!(
            cp,
            Err(NetError::RemoteStatus {
                status: StatusCode::FORBIDDEN,
                ..
            })
        ));
        assert!(cb_srv.latest_clip.lock().unwrap().is_none());

        // 3. Rogue file upload rejected before saving anything
        let rogue_file = _d_rogue.path().join("evil.txt");
        std::fs::write(&rogue_file, b"malware").unwrap();
        let up = rogue_client
            .upload_file_path(&srv_target, "127.0.0.1", &rogue_file, "b1", 1, 1, None, None)
            .await;
        assert!(matches!(
            up,
            Err(NetError::RemoteStatus {
                status: StatusCode::FORBIDDEN,
                ..
            })
        ));
        assert!(cb_srv.received_files.lock().unwrap().is_empty());
        assert!(!cb_srv.recv_dir.join("evil.txt").exists());

        // 4. Even TrustedOnly TLS policy rejects at handshake level cleanly
        let (_d_strict, id_strict, _t_strict, _p_strict, _cb_strict, srv_strict, tx_strict) =
            spawn_node("StrictPC", Platform::Linux, TlsAuthPolicy::TrustedOnly).await;
        let strict_target = as_trusted_target(&id_strict, srv_strict.local_addr());
        let strict_err = rogue_client.heartbeat(&strict_target, "127.0.0.1", 45877).await;
        assert!(strict_err.is_err());

        // Prevent unused warning
        let _ = trust_srv;
        let _ = tx_srv.send(true);
        let _ = tx_r.send(true);
        let _ = tx_strict.send(true);
    }

    #[tokio::test]
    async fn full_sas_pairing_clipboard_file_and_revocation_flow() {
        let (_d_pc, id_pc, trust_pc, pair_pc, cb_pc, srv_pc, tx_pc) =
            spawn_node("ArchPC", Platform::Linux, TlsAuthPolicy::AllowPairingRequests).await;
        let (d_ph, id_ph, trust_ph, pair_ph, cb_ph, _srv_ph, tx_ph) =
            spawn_node("OPPO-PLP110", Platform::Android, TlsAuthPolicy::AllowPairingRequests).await;

        let disc_pc = as_discovered(&id_pc, srv_pc.local_addr());

        // Step A: Phone initiates SAS pairing to PC
        let ph_view = pair_ph.start_outgoing_pairing(&disc_pc).await.unwrap();
        let pc_pending = pair_pc.list_pending();
        assert_eq!(pc_pending.len(), 1);
        assert_eq!(pc_pending[0].sas_code, ph_view.sas_code);
        assert_eq!(cb_pc.pair_requests.lock().unwrap().len(), 1);

        // Phone approves locally FIRST: must NOT be trusted yet until PC approves!
        let ph_after_local = pair_ph
            .approve_local(&ph_view.session_id, Some(cb_ph.as_ref()))
            .await
            .unwrap();
        assert_eq!(ph_after_local.state, PairState::ApprovedLocal);
        assert!(!trust_ph.is_pubkey_trusted(&id_pc.pubkey_bytes()));
        assert!(!trust_pc.is_pubkey_trusted(&id_ph.pubkey_bytes()));

        // Step B: PC approves matching SAS code -> both sides complete & persist trust!
        let pc_done = pair_pc
            .approve_local(&ph_view.session_id, Some(cb_pc.as_ref()))
            .await
            .unwrap();
        assert_eq!(pc_done.state, PairState::Completed);
        assert!(trust_pc.is_pubkey_trusted(&id_ph.pubkey_bytes()));

        let ph_done = pair_ph
            .poll_outgoing_pair(&ph_view.session_id, Some(cb_ph.as_ref()))
            .await
            .unwrap();
        assert_eq!(ph_done.state, PairState::Completed);
        assert!(trust_ph.is_pubkey_trusted(&id_pc.pubkey_bytes()));

        // Fix responder-stored port for test client calls (initiator advertised tls_port=0 in PairRequestPayload).
        let mut trusted_pc = trust_ph.get_by_device_id(&id_pc.device_id).unwrap();
        trusted_pc.tls_port = srv_pc.local_addr().port();

        // Step C: Paired phone pushes text (including Base64-like string -> remains Text!), pulls back, and sends CJK file
        let phone_client = MtlsClient::new(id_ph.clone());
        let base64_text = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwC";
        let clip_msg = ClipboardMessage::new_text(&id_ph.device_id, 1, base64_text).unwrap();
        let ack = phone_client
            .push_clipboard(&trusted_pc, "127.0.0.1", &clip_msg)
            .await
            .unwrap();
        assert!(ack.applied);

        let pulled = phone_client
            .pull_clipboard(&trusted_pc, "127.0.0.1")
            .await
            .unwrap();
        assert_eq!(pulled.kind, crate::clipboard_model::ClipKind::Text);
        assert_eq!(pulled.data, base64_text);

        let test_file = d_ph.path().join("寰宇 测试照片.png");
        let payload_bytes = vec![0x42u8; 256 * 1024 + 77];
        std::fs::write(&test_file, &payload_bytes).unwrap();
        let upload_ack = phone_client
            .upload_file_path(
                &trusted_pc,
                "127.0.0.1",
                &test_file,
                "batch-1",
                1,
                1,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(upload_ack.file_name, "寰宇 测试照片.png");
        assert_eq!(upload_ack.size, payload_bytes.len() as u64);
        assert_eq!(std::fs::read(&upload_ack.saved_path).unwrap(), payload_bytes);

        // Step D: Wrong server pubkey pin rejected at TLS level
        let wrong_client_target = TrustedPeer {
            pubkey_b64: id_ph.pubkey_b64(), // wrong pubkey for srv_pc!
            ..trusted_pc.clone()
        };
        assert!(
            phone_client
                .heartbeat(&wrong_client_target, "127.0.0.1", 45877)
                .await
                .is_err()
        );

        // Step E: Revoke trust on PC -> next call from previously paired phone immediately fails with 403!
        trust_pc.remove(&id_ph.device_id).unwrap();
        let after_revoke = phone_client
            .heartbeat(&trusted_pc, "127.0.0.1", 45877)
            .await;
        assert!(matches!(
            after_revoke,
            Err(NetError::RemoteStatus {
                status: StatusCode::FORBIDDEN,
                ..
            })
        ));

        let _ = tx_pc.send(true);
        let _ = tx_ph.send(true);
    }
}
