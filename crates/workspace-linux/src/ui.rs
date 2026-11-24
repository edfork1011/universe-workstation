//! Native GTK4 Liquid Glass UI bridge running GTK4 on main thread and Tokio in background.

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use workspace_core::clipboard_model::ClipKind;
use workspace_core::limits::format_human_size;
use workspace_core::mtls_http::PeerCallbacks;
use workspace_core::protocol::{PairState, TransferDirection, TransferState};

use crate::daemon::{DaemonRuntime, DaemonSnapshot, UiEvent, open_folder_xdg};

static LOGO_128_PNG: &[u8] = include_bytes!("../../../assets/logo_128.png");

#[repr(C)]
struct WsUiStateOpaque {
    _private: [u8; 0],
}

#[repr(C)]
struct WsUiCallbacks {
    on_startup_ready: extern "C" fn(*mut c_void),
    on_pair_start: extern "C" fn(*const c_char, *mut c_void),
    on_pair_approve: extern "C" fn(*const c_char, *mut c_void),
    on_pair_reject: extern "C" fn(*const c_char, *mut c_void),
    on_unpair: extern "C" fn(*const c_char, *mut c_void),
    on_set_sync_paused: extern "C" fn(c_int, *mut c_void),
    on_send_files: extern "C" fn(*const c_char, *const *const c_char, c_int, *mut c_void),
    on_open_downloads: extern "C" fn(*mut c_void),
    on_set_user_name: extern "C" fn(*const c_char, *mut c_void),
    on_set_device_name: extern "C" fn(*const c_char, *mut c_void),
    on_quit_clicked: extern "C" fn(*mut c_void),
}

unsafe extern "C" {
    fn ws_ui_new(
        start_silent: c_int,
        display_user: *const c_char,
        logo_png: *const u8,
        logo_len: usize,
        cb: WsUiCallbacks,
        user_data: *mut c_void,
    ) -> *mut WsUiStateOpaque;
    fn ws_ui_run(st: *mut WsUiStateOpaque) -> c_int;
    fn ws_ui_present(st: *mut WsUiStateOpaque, startup_id: *const c_char);
    fn ws_ui_quit(st: *mut WsUiStateOpaque);
    fn ws_ui_set_header(
        st: *mut WsUiStateOpaque,
        user_name: *const c_char,
        device_name: *const c_char,
        fingerprint: *const c_char,
        tls_port: c_int,
        receive_dir: *const c_char,
        sync_paused: c_int,
        online_count: c_int,
    );
    fn ws_ui_clear_sas(st: *mut WsUiStateOpaque);
    fn ws_ui_add_sas(
        st: *mut WsUiStateOpaque,
        session_id: *const c_char,
        peer_name: *const c_char,
        fingerprint: *const c_char,
        sas_code: *const c_char,
        is_initiator: c_int,
        approved_local: c_int,
    );
    fn ws_ui_clear_paired(st: *mut WsUiStateOpaque);
    fn ws_ui_add_paired(
        st: *mut WsUiStateOpaque,
        device_id: *const c_char,
        device_name: *const c_char,
        connected: c_int,
        ip: *const c_char,
        tls_port: c_int,
        rtt_ms: i64,
        fingerprint: *const c_char,
    );
    fn ws_ui_update_paired_rtt(
        st: *mut WsUiStateOpaque,
        index: c_int,
        connected: c_int,
        rtt_ms: i64,
    );
    fn ws_ui_finish_paired(st: *mut WsUiStateOpaque);
    fn ws_ui_clear_discovered(st: *mut WsUiStateOpaque);
    fn ws_ui_add_discovered(
        st: *mut WsUiStateOpaque,
        device_id: *const c_char,
        device_name: *const c_char,
        ip: *const c_char,
        tls_port: c_int,
        fingerprint: *const c_char,
        is_trusted: c_int,
    );
    fn ws_ui_finish_discovered(st: *mut WsUiStateOpaque);
    fn ws_ui_set_clipboard_summary(
        st: *mut WsUiStateOpaque,
        has_summary: c_int,
        direction: *const c_char,
        peer_name: *const c_char,
        kind: *const c_char,
        size_label: *const c_char,
    );
    fn ws_ui_clear_transfers(st: *mut WsUiStateOpaque);
    fn ws_ui_add_transfer(
        st: *mut WsUiStateOpaque,
        summary: *const c_char,
        peer_name: *const c_char,
        direction: *const c_char,
        bytes_label: *const c_char,
        state_label: *const c_char,
        state_kind: *const c_char,
        fraction: c_double,
        error_msg: *const c_char,
    );
    fn ws_ui_finish_transfers(st: *mut WsUiStateOpaque);
    fn ws_schedule_idle(func: extern "C" fn(*mut c_void), arg: *mut c_void);
}

#[derive(Debug)]
pub enum UiCommand {
    PairStart(String),
    PairApprove(String),
    PairReject(String),
    Unpair(String),
    SetSyncPaused(bool),
    SendFiles(String, Vec<PathBuf>),
    OpenDownloads,
    SetUserName(String),
    SetDeviceName(String),
    Quit,
}

#[derive(Default)]
struct PendingGtkEvents {
    events: Vec<UiEvent>,
    ready: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct HeaderVm {
    user_name: String,
    device_name: String,
    fingerprint: String,
    tls_port: u16,
    receive_dir: String,
    sync_paused: bool,
    online_count: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct SasVm {
    session_id: String,
    peer_name: String,
    peer_fingerprint: String,
    sas_code: String,
    is_initiator: bool,
    approved_local: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct PairedTopologyVm {
    device_id: String,
    device_name: String,
    connected: bool,
    ip: Option<String>,
    tls_port: u16,
    fingerprint: String,
}

#[derive(Clone, PartialEq, Eq)]
struct DiscoveredVm {
    device_id: String,
    device_name: String,
    ip: String,
    tls_port: u16,
    fingerprint: String,
    is_trusted: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct ClipboardVm {
    direction: TransferDirection,
    peer_name: String,
    kind: ClipKind,
    size: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct TransferVm {
    id: String,
    summary: String,
    peer_name: String,
    direction: TransferDirection,
    bytes_transferred: u64,
    total_bytes: u64,
    state: TransferState,
    error_message: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct RenderedViewModel {
    header: HeaderVm,
    sas: Vec<SasVm>,
    paired_topology: Vec<PairedTopologyVm>,
    paired_rtts: Vec<(bool, i64)>,
    discovered: Vec<DiscoveredVm>,
    clipboard: Option<ClipboardVm>,
    transfers: Vec<TransferVm>,
}

impl RenderedViewModel {
    fn from_snapshot(snap: &DaemonSnapshot) -> Self {
        let online_count = snap.paired_peers.iter().filter(|p| p.connected).count();
        Self {
            header: HeaderVm {
                user_name: snap.user_name.clone(),
                device_name: snap.device_name.clone(),
                fingerprint: snap.fingerprint.clone(),
                tls_port: snap.tls_port,
                receive_dir: snap.receive_dir.clone(),
                sync_paused: snap.sync_paused,
                online_count,
            },
            sas: snap
                .pending_pairs
                .iter()
                .map(|s| SasVm {
                    session_id: s.session_id.clone(),
                    peer_name: s.peer_name.clone(),
                    peer_fingerprint: s.peer_fingerprint.clone(),
                    sas_code: s.sas_code.clone(),
                    is_initiator: s.is_initiator,
                    approved_local: s.state == PairState::ApprovedLocal,
                })
                .collect(),
            paired_topology: snap
                .paired_peers
                .iter()
                .map(|p| PairedTopologyVm {
                    device_id: p.device_id.clone(),
                    device_name: p.device_name.clone(),
                    connected: p.connected,
                    ip: p.ip.clone(),
                    tls_port: p.tls_port,
                    fingerprint: p.fingerprint.clone(),
                })
                .collect(),
            paired_rtts: snap
                .paired_peers
                .iter()
                .map(|p| (p.connected, p.rtt_ms.map(|v| v as i64).unwrap_or(-1)))
                .collect(),
            discovered: snap
                .discovered_peers
                .iter()
                .map(|d| DiscoveredVm {
                    device_id: d.device_id.clone(),
                    device_name: d.device_name.clone(),
                    ip: d.ip.clone(),
                    tls_port: d.tls_port,
                    fingerprint: d.fingerprint.clone(),
                    is_trusted: d.is_trusted,
                })
                .collect(),
            clipboard: snap.last_clipboard.as_ref().map(|c| ClipboardVm {
                direction: c.direction,
                peer_name: c.peer_name.clone(),
                kind: c.kind,
                size: c.size,
            }),
            transfers: snap
                .transfers
                .iter()
                .map(|t| TransferVm {
                    id: t.id.clone(),
                    summary: t.summary.clone(),
                    peer_name: t.peer_name.clone(),
                    direction: t.direction,
                    bytes_transferred: t.bytes_transferred,
                    total_bytes: t.total_bytes,
                    state: t.state,
                    error_message: t.error_message.clone(),
                })
                .collect(),
        }
    }
}

pub struct BridgeContext {
    ui_ptr: Mutex<*mut WsUiStateOpaque>,
    pending: Mutex<PendingGtkEvents>,
    last_rendered: Mutex<Option<RenderedViewModel>>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    runtime_ref: Mutex<Option<DaemonRuntime>>,
}

unsafe impl Send for BridgeContext {}
unsafe impl Sync for BridgeContext {}

fn to_cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_else(|_| CString::new("").unwrap())
}

fn current_display_user() -> String {
    crate::config::default_linux_user_name()
}

extern "C" fn cb_startup_ready(user_data: *mut c_void) {
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    {
        let mut p = ctx.pending.lock().expect("pending UI lock poisoned");
        p.ready = true;
    }
    if let Some(rt) = ctx.runtime_ref.lock().expect("runtime ref poisoned").as_ref() {
        let snap = rt.snapshot();
        let st = *ctx.ui_ptr.lock().unwrap();
        apply_snapshot_on_main_thread(ctx, st, &snap);
    }
    cb_drain_idle(user_data);
}

extern "C" fn cb_pair_start(device_id: *const c_char, user_data: *mut c_void) {
    if device_id.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let id = unsafe { CStr::from_ptr(device_id) }
        .to_string_lossy()
        .into_owned();
    let _ = ctx.cmd_tx.send(UiCommand::PairStart(id));
}

extern "C" fn cb_pair_approve(session_id: *const c_char, user_data: *mut c_void) {
    if session_id.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let sid = unsafe { CStr::from_ptr(session_id) }
        .to_string_lossy()
        .into_owned();
    let _ = ctx.cmd_tx.send(UiCommand::PairApprove(sid));
}

extern "C" fn cb_pair_reject(session_id: *const c_char, user_data: *mut c_void) {
    if session_id.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let sid = unsafe { CStr::from_ptr(session_id) }
        .to_string_lossy()
        .into_owned();
    let _ = ctx.cmd_tx.send(UiCommand::PairReject(sid));
}

extern "C" fn cb_unpair(device_id: *const c_char, user_data: *mut c_void) {
    if device_id.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let id = unsafe { CStr::from_ptr(device_id) }
        .to_string_lossy()
        .into_owned();
    let _ = ctx.cmd_tx.send(UiCommand::Unpair(id));
}

extern "C" fn cb_set_sync_paused(paused: c_int, user_data: *mut c_void) {
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let _ = ctx.cmd_tx.send(UiCommand::SetSyncPaused(paused != 0));
}

extern "C" fn cb_send_files(
    peer_id: *const c_char,
    paths: *const *const c_char,
    count: c_int,
    user_data: *mut c_void,
) {
    if peer_id.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let peer = unsafe { CStr::from_ptr(peer_id) }
        .to_string_lossy()
        .into_owned();
    let mut vec = Vec::with_capacity(count.max(0) as usize);
    for idx in 0..count.max(0) {
        let ptr = unsafe { *paths.offset(idx as isize) };
        if !ptr.is_null() {
            let p = unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned();
            vec.push(PathBuf::from(p));
        }
    }
    let _ = ctx.cmd_tx.send(UiCommand::SendFiles(peer, vec));
}

extern "C" fn cb_open_downloads(user_data: *mut c_void) {
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let _ = ctx.cmd_tx.send(UiCommand::OpenDownloads);
}

extern "C" fn cb_set_user_name(name: *const c_char, user_data: *mut c_void) {
    if name.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let s = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    let _ = ctx.cmd_tx.send(UiCommand::SetUserName(s));
}

extern "C" fn cb_set_device_name(name: *const c_char, user_data: *mut c_void) {
    if name.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let s = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    let _ = ctx.cmd_tx.send(UiCommand::SetDeviceName(s));
}

extern "C" fn cb_quit(user_data: *mut c_void) {
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let _ = ctx.cmd_tx.send(UiCommand::Quit);
}

extern "C" fn cb_drain_idle(user_data: *mut c_void) {
    let ctx = unsafe { &*(user_data as *const BridgeContext) };
    let st = *ctx.ui_ptr.lock().unwrap();
    if st.is_null() {
        return;
    }
    let drained: Vec<UiEvent> = {
        let mut p = ctx.pending.lock().expect("pending UI lock poisoned");
        if !p.ready {
            return;
        }
        std::mem::take(&mut p.events)
    };

    let mut latest_snap: Option<DaemonSnapshot> = None;
    let mut show_win = false;
    let mut show_token: Option<String> = None;
    let mut do_quit = false;

    for ev in drained {
        match ev {
            UiEvent::Snapshot(s) => latest_snap = Some(*s),
            UiEvent::ShowWindow(token) => {
                show_win = true;
                if token.is_some() {
                    show_token = token;
                }
            }
            UiEvent::Quit => do_quit = true,
        }
    }

    if let Some(snap) = latest_snap {
        apply_snapshot_on_main_thread(ctx, st, &snap);
    }
    if show_win {
        let tok_c = show_token.as_deref().map(to_cstr);
        let tok_ptr = tok_c.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        unsafe { ws_ui_present(st, tok_ptr) };
        if let Some(rt) = ctx.runtime_ref.lock().expect("runtime ref poisoned").as_ref() {
            apply_snapshot_on_main_thread(ctx, st, &rt.snapshot());
        }
    }
    if do_quit {
        unsafe { ws_ui_quit(st) };
    }
}

fn apply_snapshot_on_main_thread(
    ctx: &BridgeContext,
    st: *mut WsUiStateOpaque,
    snap: &DaemonSnapshot,
) {
    if st.is_null() {
        return;
    }

    let vm = RenderedViewModel::from_snapshot(snap);
    let mut prev_guard = ctx
        .last_rendered
        .lock()
        .expect("last_rendered UI lock poisoned");
    let prev = prev_guard.as_ref();

    if prev.map(|p| &p.header) != Some(&vm.header) {
        let user_name_c = to_cstr(&vm.header.user_name);
        let dev_name_c = to_cstr(&vm.header.device_name);
        let fp_c = to_cstr(&vm.header.fingerprint);
        let dir_c = to_cstr(&vm.header.receive_dir);
        unsafe {
            ws_ui_set_header(
                st,
                user_name_c.as_ptr(),
                dev_name_c.as_ptr(),
                fp_c.as_ptr(),
                vm.header.tls_port as c_int,
                dir_c.as_ptr(),
                i32::from(vm.header.sync_paused),
                vm.header.online_count as c_int,
            );
        }
    }

    if prev.map(|p| &p.sas) != Some(&vm.sas) {
        unsafe { ws_ui_clear_sas(st) };
        for sas in &vm.sas {
            let sid_c = to_cstr(&sas.session_id);
            let pname_c = to_cstr(&sas.peer_name);
            let pfp_c = to_cstr(&sas.peer_fingerprint);
            let code_c = to_cstr(&sas.sas_code);
            unsafe {
                ws_ui_add_sas(
                    st,
                    sid_c.as_ptr(),
                    pname_c.as_ptr(),
                    pfp_c.as_ptr(),
                    code_c.as_ptr(),
                    i32::from(sas.is_initiator),
                    i32::from(sas.approved_local),
                );
            }
        }
    }

    if prev.map(|p| &p.paired_topology) != Some(&vm.paired_topology) {
        unsafe { ws_ui_clear_paired(st) };
        for (topo, &(_, rtt)) in vm.paired_topology.iter().zip(vm.paired_rtts.iter()) {
            let did_c = to_cstr(&topo.device_id);
            let dname_c = to_cstr(&topo.device_name);
            let ip_c = to_cstr(topo.ip.as_deref().unwrap_or(""));
            let pfp_c = to_cstr(&topo.fingerprint);
            unsafe {
                ws_ui_add_paired(
                    st,
                    did_c.as_ptr(),
                    dname_c.as_ptr(),
                    i32::from(topo.connected),
                    ip_c.as_ptr(),
                    topo.tls_port as c_int,
                    rtt,
                    pfp_c.as_ptr(),
                );
            }
        }
        unsafe { ws_ui_finish_paired(st) };
    } else if prev.map(|p| &p.paired_rtts) != Some(&vm.paired_rtts) {
        let prev_rtts = prev.map(|p| p.paired_rtts.as_slice()).unwrap_or(&[]);
        for (idx, &(conn, rtt)) in vm.paired_rtts.iter().enumerate() {
            if prev_rtts.get(idx).copied() != Some((conn, rtt)) {
                unsafe {
                    ws_ui_update_paired_rtt(st, idx as c_int, i32::from(conn), rtt);
                }
            }
        }
    }

    if prev.map(|p| &p.discovered) != Some(&vm.discovered) {
        unsafe { ws_ui_clear_discovered(st) };
        for d in &vm.discovered {
            let did_c = to_cstr(&d.device_id);
            let dname_c = to_cstr(&d.device_name);
            let ip_c = to_cstr(&d.ip);
            let dfp_c = to_cstr(&d.fingerprint);
            unsafe {
                ws_ui_add_discovered(
                    st,
                    did_c.as_ptr(),
                    dname_c.as_ptr(),
                    ip_c.as_ptr(),
                    d.tls_port as c_int,
                    dfp_c.as_ptr(),
                    i32::from(d.is_trusted),
                );
            }
        }
        unsafe { ws_ui_finish_discovered(st) };
    }

    if prev.map(|p| &p.clipboard) != Some(&vm.clipboard) {
        if let Some(summary) = &vm.clipboard {
            let dir_txt = match summary.direction {
                TransferDirection::Send => "Linux -> 手机",
                TransferDirection::Receive => "手机 -> Linux",
            };
            let kind_txt = match summary.kind {
                ClipKind::Text => "文本",
                ClipKind::Image => "图片",
            };
            let size_txt = format_human_size(summary.size as u64);
            let dir_c = to_cstr(dir_txt);
            let peer_c = to_cstr(&summary.peer_name);
            let kind_c = to_cstr(kind_txt);
            let size_c = to_cstr(&size_txt);
            unsafe {
                ws_ui_set_clipboard_summary(
                    st,
                    1,
                    dir_c.as_ptr(),
                    peer_c.as_ptr(),
                    kind_c.as_ptr(),
                    size_c.as_ptr(),
                );
            }
        } else {
            let empty = to_cstr("");
            unsafe {
                ws_ui_set_clipboard_summary(
                    st,
                    0,
                    empty.as_ptr(),
                    empty.as_ptr(),
                    empty.as_ptr(),
                    empty.as_ptr(),
                );
            }
        }
    }

    if prev.map(|p| &p.transfers) != Some(&vm.transfers) {
        unsafe { ws_ui_clear_transfers(st) };
        for tr in &vm.transfers {
            let sum_c = to_cstr(&tr.summary);
            let peer_c = to_cstr(&tr.peer_name);
            let dir_str = match tr.direction {
                TransferDirection::Send => "发送",
                TransferDirection::Receive => "接收",
            };
            let dir_c = to_cstr(dir_str);
            let bytes_str = format!(
                "{} / {}",
                format_human_size(tr.bytes_transferred),
                format_human_size(tr.total_bytes)
            );
            let bytes_c = to_cstr(&bytes_str);
            let (st_label, st_kind) = match tr.state {
                TransferState::Transferring => ("传输中", "run"),
                TransferState::Completed => ("已完成", "ok"),
                TransferState::Failed => ("失败", "err"),
                TransferState::Cancelled => ("已取消", "off"),
            };
            let st_label_c = to_cstr(st_label);
            let st_kind_c = to_cstr(st_kind);
            let frac = if tr.total_bytes > 0 {
                (tr.bytes_transferred as f64 / tr.total_bytes as f64).clamp(0.0, 1.0)
            } else if tr.state == TransferState::Completed {
                1.0
            } else {
                0.0
            };
            let err_c = to_cstr(tr.error_message.as_deref().unwrap_or(""));
            unsafe {
                ws_ui_add_transfer(
                    st,
                    sum_c.as_ptr(),
                    peer_c.as_ptr(),
                    dir_c.as_ptr(),
                    bytes_c.as_ptr(),
                    st_label_c.as_ptr(),
                    st_kind_c.as_ptr(),
                    frac,
                    err_c.as_ptr(),
                );
            }
        }
        unsafe { ws_ui_finish_transfers(st) };
    }

    *prev_guard = Some(vm);
}

pub fn push_event_to_gtk(ctx: &Arc<BridgeContext>, ev: UiEvent) {
    {
        let mut p = ctx.pending.lock().expect("pending UI lock poisoned");
        p.events.push(ev);
    }
    let ptr = Arc::as_ptr(ctx) as *mut c_void;
    unsafe { ws_schedule_idle(cb_drain_idle, ptr) };
}

pub fn create_gtk_controller(
    start_silent: bool,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
) -> Arc<BridgeContext> {
    let ctx = Arc::new(BridgeContext {
        ui_ptr: Mutex::new(std::ptr::null_mut()),
        pending: Mutex::new(PendingGtkEvents::default()),
        last_rendered: Mutex::new(None),
        cmd_tx,
        runtime_ref: Mutex::new(None),
    });

    let cbs = WsUiCallbacks {
        on_startup_ready: cb_startup_ready,
        on_pair_start: cb_pair_start,
        on_pair_approve: cb_pair_approve,
        on_pair_reject: cb_pair_reject,
        on_unpair: cb_unpair,
        on_set_sync_paused: cb_set_sync_paused,
        on_send_files: cb_send_files,
        on_open_downloads: cb_open_downloads,
        on_set_user_name: cb_set_user_name,
        on_set_device_name: cb_set_device_name,
        on_quit_clicked: cb_quit,
    };

    let user_c = to_cstr(&current_display_user());
    let raw_ctx = Arc::as_ptr(&ctx) as *mut c_void;
    let ui_ptr = unsafe {
        ws_ui_new(
            i32::from(start_silent),
            user_c.as_ptr(),
            LOGO_128_PNG.as_ptr(),
            LOGO_128_PNG.len(),
            cbs,
            raw_ctx,
        )
    };
    *ctx.ui_ptr.lock().unwrap() = ui_ptr;
    ctx
}

pub fn run_gtk_main_loop(ctx: &Arc<BridgeContext>, runtime: DaemonRuntime) -> i32 {
    *ctx.runtime_ref.lock().unwrap() = Some(runtime);
    let ui_ptr = *ctx.ui_ptr.lock().unwrap();
    unsafe { ws_ui_run(ui_ptr) }
}

pub async fn handle_ui_command(rt: &DaemonRuntime, cmd: UiCommand) {
    match cmd {
        UiCommand::PairStart(id) => {
            if let Err(e) = rt.start_pair(&id).await {
                tracing::warn!("pair start failed: {e}");
                crate::notify::send_notification("发起配对失败", e);
            }
        }
        UiCommand::PairApprove(sid) => {
            if let Err(e) = rt.approve_pair(&sid).await {
                tracing::warn!("pair approve failed: {e}");
            }
        }
        UiCommand::PairReject(sid) => {
            let _ = rt.reject_pair(&sid).await;
        }
        UiCommand::Unpair(id) => {
            let _ = rt.unpair_peer(&id);
        }
        UiCommand::SetSyncPaused(paused) => {
            let _ = rt.set_sync_paused(paused);
        }
        UiCommand::SendFiles(peer_id, paths) => {
            let rt_clone = rt.clone();
            tokio::spawn(async move {
                if let Err(err) = rt_clone.send_files_to_peer(&peer_id, paths, None).await {
                    crate::notify::send_notification("文件发送失败", err);
                }
            });
        }
        UiCommand::OpenDownloads => {
            let dir = rt.receive_dir();
            let _ = std::fs::create_dir_all(&dir);
            open_folder_xdg(&dir);
        }
        UiCommand::SetUserName(name) => {
            if let Err(e) = rt.set_user_name(&name) {
                tracing::warn!("set_user_name failed: {e}");
            }
        }
        UiCommand::SetDeviceName(name) => {
            if let Err(e) = rt.set_device_name(&name).await {
                tracing::warn!("set_device_name failed: {e}");
            }
        }
        UiCommand::Quit => {
            rt.request_quit();
        }
    }
}
