//! UID-authenticated localhost control & SSE event server (`127.0.0.1:45878`).
//!
//! Resolves each incoming TCP peer socket's UID via `/proc/net/tcp` + `/data/system/packages.list`
//! and restricts access solely to `root`, `shell`, `me.edfork.workspace`, `com.android.systemui`,
//! and `com.coloros.oshare` with strict per-caller role permissions.

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
#[cfg(test)]
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::os::unix::fs::MetadataExt;
use std::pin::Pin;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::TryStreamExt;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::io::StreamReader;
use tower::ServiceExt;
use tracing::{debug, warn};
use uuid::Uuid;
use workspace_core::error::{ApiErrorBody, ErrorCode};
use workspace_core::mtls_http::{
    FileUploadMeta, HDR_BATCH_ID, HDR_FILE_INDEX, HDR_FILE_NAME, HDR_FILE_SIZE, HDR_FILE_TOTAL,
};
use workspace_core::transfer::percent_decode_utf8;

use crate::daemon::{AndroidDaemonRuntime, AndroidDaemonSnapshot};

const PKG_WORKSPACE: &str = "me.edfork.workspace";
const PKG_SYSTEMUI: &str = "com.android.systemui";
const PKG_OSHARE: &str = "com.coloros.oshare";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCallerRole {
    Admin,
    SystemUi,
    OShare,
}

#[derive(Debug, Clone)]
struct VerifiedLocalCaller {
    uid: u32,
    role: LocalCallerRole,
}

impl LocalCallerRole {
    fn can_admin(self) -> bool {
        matches!(self, Self::Admin)
    }

    fn can_clipboard(self) -> bool {
        matches!(self, Self::Admin | Self::SystemUi)
    }

    fn can_oshare(self) -> bool {
        matches!(self, Self::Admin | Self::OShare)
    }
}

/// Parse `/data/system/packages.list` mapping package name -> base UID.
pub fn parse_packages_list(contents: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        let Some(pkg) = parts.next() else { continue };
        let Some(uid_str) = parts.next() else { continue };
        if let Ok(uid) = uid_str.parse::<u32>() {
            map.insert(pkg.to_string(), uid);
        }
    }
    map
}

fn is_loopback_proc_v4_hex(hex: &str) -> bool {
    hex.eq_ignore_ascii_case("0100007F") || hex.eq_ignore_ascii_case("7F000001")
}

fn is_loopback_proc_v6_hex(hex: &str) -> bool {
    hex.eq_ignore_ascii_case("00000000000000000000000001000000")
        || hex.eq_ignore_ascii_case("00000000000000000000000000000001")
        || hex.eq_ignore_ascii_case("0000000000000000FFFF00000100007F")
        || hex.eq_ignore_ascii_case("00000000000000000000FFFF0100007F")
        || hex.eq_ignore_ascii_case("00000000000000000000FFFF7F000001")
}

fn parse_proc_addr_port(field: &str) -> Option<(&str, u16)> {
    let (addr_hex, port_hex) = field.split_once(':')?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    Some((addr_hex, port))
}

/// Find UID owning the *client* side of a loopback TCP connection (`local == client, rem == server`).
#[cfg(test)]
pub fn parse_proc_net_tcp_uid(
    contents: &str,
    _client_ip: Ipv4Addr,
    client_port: u16,
    server_port: u16,
) -> Option<u32> {
    parse_proc_table_uid(contents, client_port, server_port, is_loopback_proc_v4_hex)
}

fn parse_proc_table_uid(
    contents: &str,
    client_port: u16,
    server_port: u16,
    is_loopback_hex: fn(&str) -> bool,
) -> Option<u32> {
    for line in contents.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 8 {
            continue;
        }
        let Some((local_hex, local_port)) = parse_proc_addr_port(cols[1]) else {
            continue;
        };
        let Some((rem_hex, rem_port)) = parse_proc_addr_port(cols[2]) else {
            continue;
        };
        if local_port == client_port
            && rem_port == server_port
            && is_loopback_hex(local_hex)
            && is_loopback_hex(rem_hex)
            && let Ok(uid) = cols[7].parse::<u32>()
        {
            return Some(uid);
        }
    }
    None
}

/// Search both `/proc/net/tcp` and `/proc/net/tcp6` (Android Java `HttpURLConnection` uses dual-stack `AF_INET6` sockets mapped as `::ffff:127.0.0.1` even when talking to an IPv4 listener).
pub fn find_loopback_socket_uid(
    tcp_v4: Option<&str>,
    tcp_v6: Option<&str>,
    client_port: u16,
    server_port: u16,
) -> Option<u32> {
    if let Some(v4) = tcp_v4
        && let Some(uid) = parse_proc_table_uid(v4, client_port, server_port, is_loopback_proc_v4_hex)
    {
        return Some(uid);
    }
    if let Some(v6) = tcp_v6
        && let Some(uid) = parse_proc_table_uid(v6, client_port, server_port, is_loopback_proc_v6_hex)
    {
        return Some(uid);
    }
    None
}

/// Resolve OS UID for an accepted loopback TCP client socket.
fn resolve_loopback_client_uid(peer_addr: SocketAddr, server_port: u16) -> Option<u32> {
    let tcp_v4 = std::fs::read_to_string("/proc/net/tcp").ok();
    let tcp_v6 = std::fs::read_to_string("/proc/net/tcp6").ok();
    find_loopback_socket_uid(
        tcp_v4.as_deref(),
        tcp_v6.as_deref(),
        peer_addr.port(),
        server_port,
    )
}

fn fallback_uid_for_package(pkg: &str) -> Option<u32> {
    for base in ["/data/user_de/0", "/data/user/0", "/data/data"] {
        let p = std::path::Path::new(base).join(pkg);
        if let Ok(meta) = std::fs::metadata(&p) {
            return Some(meta.uid());
        }
    }
    None
}

/// Authorize a resolved Linux/Android UID cleanly against Android's multi-user appId (`uid % 100_000`).
pub fn authorize_uid(uid: u32, packages_list_contents: &str) -> Option<LocalCallerRole> {
    if uid == 0 || uid == 2000 {
        return Some(LocalCallerRole::Admin);
    }
    let app_id = uid % 100_000;
    let map = parse_packages_list(packages_list_contents);
    if map.get(PKG_WORKSPACE).copied().map(|u| u % 100_000) == Some(app_id)
        || fallback_uid_for_package(PKG_WORKSPACE).map(|u| u % 100_000) == Some(app_id)
    {
        return Some(LocalCallerRole::Admin);
    }
    if map.get(PKG_SYSTEMUI).copied().map(|u| u % 100_000) == Some(app_id)
        || fallback_uid_for_package(PKG_SYSTEMUI).map(|u| u % 100_000) == Some(app_id)
    {
        return Some(LocalCallerRole::SystemUi);
    }
    if map.get(PKG_OSHARE).copied().map(|u| u % 100_000) == Some(app_id)
        || fallback_uid_for_package(PKG_OSHARE).map(|u| u % 100_000) == Some(app_id)
    {
        return Some(LocalCallerRole::OShare);
    }
    None
}

fn check_client_caller(peer_addr: SocketAddr, server_port: u16) -> Result<VerifiedLocalCaller, &'static str> {
    if !peer_addr.ip().is_loopback() {
        return Err("non-loopback local IPC connection rejected");
    }
    let Some(uid) = resolve_loopback_client_uid(peer_addr, server_port) else {
        return Err("unable to resolve client UID from /proc/net/tcp");
    };

    #[cfg(not(target_os = "android"))]
    {
        let current = unsafe { libc::geteuid() };
        if uid == current {
            return Ok(VerifiedLocalCaller {
                uid,
                role: LocalCallerRole::Admin,
            });
        }
    }

    let pkg_contents = std::fs::read_to_string("/data/system/packages.list").unwrap_or_default();
    let Some(role) = authorize_uid(uid, &pkg_contents) else {
        return Err("calling app UID is not authorized for Workspace IPC");
    };
    Ok(VerifiedLocalCaller { uid, role })
}

#[derive(Clone)]
struct LocalAppState {
    rt: AndroidDaemonRuntime,
}

#[derive(Clone)]
struct CallerScopedService {
    router: Router,
    caller: VerifiedLocalCaller,
}

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for CallerScopedService {
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, mut req: http::Request<hyper::body::Incoming>) -> Self::Future {
        req.extensions_mut().insert(self.caller.clone());
        let router = self.router.clone();
        Box::pin(async move {
            let axum_req = req.map(Body::new);
            router.oneshot(axum_req).await
        })
    }
}

pub async fn start_local_ipc_server(
    rt: AndroidDaemonRuntime,
    port: u16,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let bind_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(bind_addr).await?;
    let local_port = listener.local_addr()?.port();

    let state = LocalAppState { rt };
    let router = Router::new()
        .route("/v1/local/status", get(handle_status))
        .route("/v1/local/events", get(handle_events))
        .route("/v1/local/clipboard/push", post(handle_clipboard_push))
        .route("/v1/local/clipboard/pull", post(handle_clipboard_pull))
        .route("/v1/local/oshare/send", post(handle_oshare_send))
        .route("/v1/local/oshare/cancel", post(handle_oshare_cancel))
        .route("/v1/local/pair/start", post(handle_pair_start))
        .route("/v1/local/pair/approve", post(handle_pair_approve))
        .route("/v1/local/pair/reject", post(handle_pair_reject))
        .route("/v1/local/unpair", post(handle_unpair))
        .route("/v1/local/config", post(handle_update_config))
        .with_state(state);

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                accept_res = listener.accept() => {
                    let (tcp, peer_addr) = match accept_res {
                        Ok(pair) => pair,
                        Err(err) => {
                            warn!("local IPC accept error: {err}");
                            continue;
                        }
                    };
                    let caller = match check_client_caller(peer_addr, local_port) {
                        Ok(c) => c,
                        Err(reason) => {
                            warn!("rejected local IPC client {peer_addr}: {reason}");
                            drop(tcp);
                            continue;
                        }
                    };
                    let _ = tcp.set_nodelay(true);
                    let service = CallerScopedService {
                        router: router.clone(),
                        caller,
                    };
                    let mut conn_shutdown = shutdown_rx.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(tcp);
                        let conn = http1::Builder::new().serve_connection(io, service);
                        tokio::pin!(conn);
                        tokio::select! {
                            res = &mut conn => {
                                if let Err(err) = res {
                                    debug!("local IPC conn closed: {err}");
                                }
                            }
                            _ = conn_shutdown.changed() => {
                                if *conn_shutdown.borrow() {
                                    conn.as_mut().graceful_shutdown();
                                }
                            }
                        }
                    });
                }
            }
        }
    });

    Ok(join)
}

fn forbidden(msg: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiErrorBody::new(ErrorCode::Unauthorized, msg)),
    )
        .into_response()
}

fn bad_request(code: ErrorCode, msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(ApiErrorBody::new(code, msg))).into_response()
}

async fn handle_status(State(state): State<LocalAppState>) -> Json<AndroidDaemonSnapshot> {
    Json(state.rt.snapshot())
}

async fn handle_events(State(state): State<LocalAppState>) -> Response {
    // Pure Tokio mpsc-backed SSE stream without extra dependencies.
    let (tx, rx_body) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
    let rt = state.rt.clone();
    tokio::spawn(async move {
        let mut sub = rt.subscribe_events();
        let mut snap = rt.snapshot();
        loop {
            let json = serde_json::to_string(&snap).unwrap_or_else(|_| "{}".to_string());
            let frame = format!("event: state\ndata: {json}\n\n");
            if tx.send(Ok(Bytes::from(frame))).await.is_err() {
                break;
            }
            tokio::select! {
                res = sub.recv() => {
                    match res {
                        Ok(next) => snap = next,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            snap = rt.snapshot();
                        }
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(20)) => {
                    if tx.send(Ok(Bytes::from_static(b": keepalive\n\n"))).await.is_err() {
                        break;
                    }
                    snap = rt.snapshot();
                }
            }
        }
    });

    let byte_stream = StreamFromReceiver(rx_body);
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(http::header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(byte_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

struct StreamFromReceiver(tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>);

impl futures_util::Stream for StreamFromReceiver {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

#[derive(Deserialize, Default)]
struct PeerOptionalReq {
    peer_id: Option<String>,
}

async fn handle_clipboard_push(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    body: Option<Json<PeerOptionalReq>>,
) -> Response {
    if !caller.role.can_clipboard() {
        return forbidden("clipboard endpoint restricted to SystemUI/Workspace");
    }
    let peer_id = body.and_then(|j| j.0.peer_id);
    match state.rt.push_clipboard_to_peer(peer_id.as_deref()).await {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
        Err(err) => bad_request(ErrorCode::ClipboardReadFailed, &err),
    }
}

async fn handle_clipboard_pull(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    body: Option<Json<PeerOptionalReq>>,
) -> Response {
    if !caller.role.can_clipboard() {
        return forbidden("clipboard endpoint restricted to SystemUI/Workspace");
    }
    let peer_id = body.and_then(|j| j.0.peer_id);
    match state.rt.pull_clipboard_from_peer(peer_id.as_deref()).await {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
        Err(err) => bad_request(ErrorCode::ClipboardWriteFailed, &err),
    }
}

fn header_val(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

async fn handle_oshare_send(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !caller.role.can_oshare() {
        return forbidden("oshare endpoint restricted to OShare/Workspace");
    }

    let Some(peer_id) = header_val(&headers, "x-workspace-peer-id") else {
        return bad_request(ErrorCode::InvalidMessage, "missing X-Workspace-Peer-Id");
    };
    let batch_id = header_val(&headers, HDR_BATCH_ID).unwrap_or_else(|| Uuid::new_v4().to_string());
    let file_name_raw = header_val(&headers, HDR_FILE_NAME).unwrap_or_else(|| "file".to_string());
    let file_name = percent_decode_utf8(&file_name_raw).unwrap_or(file_name_raw);
    let total_size = header_val(&headers, HDR_FILE_SIZE)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let file_index = header_val(&headers, HDR_FILE_INDEX)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let file_total = header_val(&headers, HDR_FILE_TOTAL)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);

    let byte_stream = body
        .into_data_stream()
        .map_err(std::io::Error::other);
    let reader = StreamReader::new(byte_stream);

    match state
        .rt
        .proxy_oshare_stream(
            &peer_id,
            FileUploadMeta {
                batch_id,
                file_name,
                total_size,
                file_index,
                file_total,
                blake3_hex: None,
            },
            reader,
        )
        .await
    {
        Ok(ack) => (StatusCode::OK, Json(ack)).into_response(),
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(ApiErrorBody::new(ErrorCode::TransferFailed, err)),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct CancelOShareReq {
    id: String,
}

async fn handle_oshare_cancel(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    Json(req): Json<CancelOShareReq>,
) -> Response {
    if !caller.role.can_oshare() {
        return forbidden("oshare endpoint restricted to OShare/Workspace");
    }
    state.rt.cancel_oshare(&req.id);
    Json(serde_json::json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct DeviceIdReq {
    device_id: String,
}

#[derive(Deserialize)]
struct SessionIdReq {
    session_id: String,
}

#[derive(Deserialize, Serialize)]
struct OkPairResp<T: Serialize> {
    ok: bool,
    session: T,
}

async fn handle_pair_start(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    Json(req): Json<DeviceIdReq>,
) -> Response {
    if !caller.role.can_admin() {
        return forbidden(&format!("admin route denied for uid {}", caller.uid));
    }
    match state.rt.start_pair(&req.device_id).await {
        Ok(session) => (StatusCode::OK, Json(OkPairResp { ok: true, session })).into_response(),
        Err(err) => bad_request(ErrorCode::PairingFailed, &err),
    }
}

async fn handle_pair_approve(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    Json(req): Json<SessionIdReq>,
) -> Response {
    if !caller.role.can_admin() {
        return forbidden("admin route denied");
    }
    match state.rt.approve_pair(&req.session_id).await {
        Ok(session) => (StatusCode::OK, Json(OkPairResp { ok: true, session })).into_response(),
        Err(err) => bad_request(ErrorCode::PairingFailed, &err),
    }
}

async fn handle_pair_reject(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    Json(req): Json<SessionIdReq>,
) -> Response {
    if !caller.role.can_admin() {
        return forbidden("admin route denied");
    }
    match state.rt.reject_pair(&req.session_id).await {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(err) => bad_request(ErrorCode::PairingFailed, &err),
    }
}

async fn handle_unpair(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    Json(req): Json<DeviceIdReq>,
) -> Response {
    if !caller.role.can_admin() {
        return forbidden("admin route denied");
    }
    match state.rt.unpair_peer(&req.device_id) {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(err) => bad_request(ErrorCode::InternalError, &err),
    }
}

#[derive(Deserialize)]
struct ConfigUpdateReq {
    sync_paused: Option<bool>,
    gesture_enabled: Option<bool>,
    gesture_fingers: Option<u8>,
    swipe_up_action: Option<String>,
    swipe_down_action: Option<String>,
    device_name: Option<String>,
    user_name: Option<String>,
    system_device_name: Option<String>,
}

async fn handle_update_config(
    State(state): State<LocalAppState>,
    Extension(caller): Extension<VerifiedLocalCaller>,
    Json(req): Json<ConfigUpdateReq>,
) -> Response {
    if !caller.role.can_admin() {
        return forbidden("admin route denied");
    }
    let mut new_identity_name: Option<String> = None;
    let res = state.rt.update_config(|cfg| {
        if let Some(v) = req.sync_paused {
            cfg.sync_paused = v;
        }
        if let Some(v) = req.gesture_enabled {
            cfg.gesture_enabled = v;
        }
        if let Some(v) = req.gesture_fingers {
            cfg.gesture_fingers = v.clamp(2, 4);
        }
        if let Some(v) = req.swipe_up_action {
            cfg.swipe_up_action = v;
        }
        if let Some(v) = req.swipe_down_action {
            cfg.swipe_down_action = v;
        }
        if let Some(raw_user) = req.user_name {
            let cleaned: String = raw_user
                .replace(['\r', '\n'], " ")
                .trim()
                .chars()
                .take(64)
                .collect();
            if cleaned.is_empty() {
                cfg.user_name_customized = false;
                cfg.user_name = crate::config::default_android_user_name();
            } else {
                cfg.user_name_customized = true;
                cfg.user_name = cleaned;
            }
        }
        if let Some(raw_dev) = req.device_name {
            let cleaned: String = raw_dev
                .replace(['\r', '\n'], " ")
                .trim()
                .chars()
                .take(64)
                .collect();
            if cleaned.is_empty() {
                cfg.device_name_customized = false;
                cfg.device_name = crate::config::detect_android_device_name();
            } else {
                cfg.device_name_customized = true;
                cfg.device_name = cleaned;
            }
            new_identity_name = Some(cfg.device_name.clone());
        } else if let Some(sys_dev) = req.system_device_name
            && !cfg.device_name_customized
            && let Some(cleaned) = crate::config::clean_android_value(&sys_dev)
            && cfg.device_name != cleaned
        {
            cfg.device_name = cleaned;
            new_identity_name = Some(cfg.device_name.clone());
        }
    });
    match res {
        Ok(()) => {
            if let Some(name) = new_identity_name {
                state.rt.apply_identity_name_change(name).await;
            }
            Json(serde_json::json!({"ok": true, "snapshot": state.rt.snapshot()})).into_response()
        }
        Err(err) => bad_request(ErrorCode::InternalError, &err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_net_tcp_caller_uid_accurately() {
        let sample = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:B336 0100007F:D4A2 01 00000000:00000000 00:00000000 00000000     0        0 101\n\
   1: 0100007F:D4A2 0100007F:B336 01 00000000:00000000 00:00000000 00000000 10299        0 102\n";
        // Server port 0xB336 = 45878, Client port 0xD4A2 = 54434
        let uid = parse_proc_net_tcp_uid(sample, Ipv4Addr::LOCALHOST, 54434, 45878);
        assert_eq!(uid, Some(10299));
    }

    #[test]
    fn authorizes_only_expected_uids_and_roles() {
        let pkgs = "\
com.android.systemui 10042 0 /data/user/0/com.android.systemui default:privapp\n\
com.coloros.oshare 10188 0 /data/user/0/com.coloros.oshare default:privapp\n\
me.edfork.workspace 10311 0 /data/user/0/me.edfork.workspace default\n\
com.evil.spyware 10999 0 /data/user/0/com.evil.spyware default\n";

        assert_eq!(authorize_uid(0, pkgs), Some(LocalCallerRole::Admin));
        assert_eq!(authorize_uid(2000, pkgs), Some(LocalCallerRole::Admin));
        assert_eq!(authorize_uid(10311, pkgs), Some(LocalCallerRole::Admin));
        assert_eq!(authorize_uid(1010311, pkgs), Some(LocalCallerRole::Admin)); // work profile user 10
        assert_eq!(authorize_uid(10042, pkgs), Some(LocalCallerRole::SystemUi));
        assert_eq!(authorize_uid(10188, pkgs), Some(LocalCallerRole::OShare));
        assert_eq!(authorize_uid(10999, pkgs), None);
        assert_eq!(authorize_uid(12345, pkgs), None);
    }

    #[test]
    fn resolves_ipv4_mapped_ipv6_client_socket_in_tcp6() {
        let tcp_v4 = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:B336 0100007F:D4A2 01 00000000:00000000 00:00000000 00000000     0        0 201\n";
        let tcp_v6 = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0000000000000000FFFF00000100007F:D4A2 0000000000000000FFFF00000100007F:B336 01 00000000:00000000 00:00000000 00000000 10311        0 202\n";
        let uid = find_loopback_socket_uid(Some(tcp_v4), Some(tcp_v6), 54434, 45878);
        assert_eq!(uid, Some(10311));
    }
}
