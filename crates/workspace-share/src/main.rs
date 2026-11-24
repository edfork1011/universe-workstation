// SPDX-License-Identifier: MIT
//! `workspace-share`: GTK4/Libadwaita quick-send dialog invoked by the Nautilus extension or CLI,
//! sending selected files strictly over LAN mTLS via `me.edfork.workspace.sock`.

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use workspace_core::limits::format_human_size;
use workspace_core::protocol::{PairedPeerStatus, TransferRecord, TransferState};

#[repr(C)]
struct WsShareUiOpaque {
    _private: [u8; 0],
}

#[repr(C)]
struct WsShareCallbacks {
    on_ready: extern "C" fn(*mut c_void),
    on_device_clicked: extern "C" fn(*const c_char, *mut c_void),
    on_cancel_clicked: extern "C" fn(*mut c_void),
}

unsafe extern "C" {
    fn ws_share_ui_new(cb: WsShareCallbacks, user_data: *mut c_void) -> *mut WsShareUiOpaque;
    fn ws_share_ui_run(ui: *mut WsShareUiOpaque) -> c_int;
    fn ws_share_set_file_summary(ui: *mut WsShareUiOpaque, summary: *const c_char);
    fn ws_share_set_empty_message(ui: *mut WsShareUiOpaque, msg: *const c_char);
    fn ws_share_clear_devices(ui: *mut WsShareUiOpaque);
    fn ws_share_add_device(
        ui: *mut WsShareUiOpaque,
        peer_id: *const c_char,
        name: *const c_char,
        subtitle: *const c_char,
    );
    fn ws_share_show_transfer(
        ui: *mut WsShareUiOpaque,
        title: *const c_char,
        subtitle: *const c_char,
        fraction: c_double,
        finished: c_int,
        error: c_int,
    );
    fn ws_share_schedule_idle(func: extern "C" fn(*mut c_void), arg: *mut c_void);
}

#[derive(Deserialize)]
struct DaemonStateEnvelope {
    #[serde(rename = "type")]
    kind: String,
    snapshot: Option<SnapshotLite>,
    record: Option<TransferRecord>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct SnapshotLite {
    paired_peers: Vec<PairedPeerStatus>,
}

#[derive(Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum ShareIpcCmd<'a> {
    GetState,
    SendFiles {
        peer_id: &'a str,
        paths: &'a [PathBuf],
        stream: bool,
    },
    CancelTransfer {
        batch_id: &'a str,
    },
}

enum UiUpdate {
    Devices(Result<Vec<PairedPeerStatus>, String>),
    Transfer(TransferRecord),
    TransferError(String),
}

struct AppCtx {
    ui: Mutex<*mut WsShareUiOpaque>,
    files: Vec<PathBuf>,
    pending: Mutex<Vec<UiUpdate>>,
    active_batch_id: Mutex<Option<String>>,
    transfer_started: AtomicBool,
    stop_polling: AtomicBool,
}

unsafe impl Send for AppCtx {}
unsafe impl Sync for AppCtx {}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_else(|_| CString::new("").unwrap())
}

fn socket_path() -> PathBuf {
    if let Some(dir) = dirs::runtime_dir() {
        return dir.join("me.edfork.workspace.sock");
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/me.edfork.workspace-{uid}.sock"))
}

static AUTOSTART_ATTEMPTED: AtomicBool = AtomicBool::new(false);

fn locate_daemon_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("me-edfork-workspace");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let local_bin = home.join(".local/bin/me-edfork-workspace");
        if local_bin.is_file() {
            return Some(local_bin);
        }
    }
    for sys in [
        "/usr/local/bin/me-edfork-workspace",
        "/usr/bin/me-edfork-workspace",
    ] {
        let p = PathBuf::from(sys);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn try_autostart_daemon_once() -> bool {
    if AUTOSTART_ATTEMPTED.swap(true, Ordering::SeqCst) {
        return false;
    }
    let Some(bin) = locate_daemon_binary() else {
        return false;
    };
    let Ok(_) = Command::new(bin)
        .arg("--silent")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    std::thread::sleep(Duration::from_millis(400));
    true
}

fn connect_daemon_socket() -> std::io::Result<UnixStream> {
    let sock = socket_path();
    match UnixStream::connect(&sock) {
        Ok(stream) => Ok(stream),
        Err(first_err) => {
            if try_autostart_daemon_once() {
                UnixStream::connect(&sock)
            } else {
                Err(first_err)
            }
        }
    }
}

fn ipc_query_peers() -> Result<Vec<PairedPeerStatus>, String> {
    let mut stream = connect_daemon_socket()
        .map_err(|_| "未检测到 寰宇工作台 后台运行，请先启动 me-edfork-workspace".to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let line = serde_json::to_string(&ShareIpcCmd::GetState).unwrap() + "\n";
    stream
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    reader
        .read_line(&mut resp_line)
        .map_err(|e| e.to_string())?;
    let env: DaemonStateEnvelope =
        serde_json::from_str(&resp_line).map_err(|e| e.to_string())?;
    if env.kind == "error" {
        return Err(env.message.unwrap_or_else(|| "后台返回错误".into()));
    }
    let peers = env
        .snapshot
        .map(|s| s.paired_peers.into_iter().filter(|p| p.connected).collect())
        .unwrap_or_default();
    Ok(peers)
}

fn push_update(ctx: &Arc<AppCtx>, upd: UiUpdate) {
    ctx.pending.lock().unwrap().push(upd);
    let ptr = Arc::as_ptr(ctx) as *mut c_void;
    unsafe { ws_share_schedule_idle(cb_idle_apply, ptr) };
}

extern "C" fn cb_ready(user_data: *mut c_void) {
    let ctx = unsafe { Arc::from_raw(user_data as *const AppCtx) };
    let ctx_poll = ctx.clone();
    std::mem::forget(ctx);

    let ui = *ctx_poll.ui.lock().unwrap();
    let total_size: u64 = ctx_poll
        .files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();
    let sum_text = if ctx_poll.files.len() == 1 {
        let name = ctx_poll.files[0]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        format!("准备发送：{name} ({})", format_human_size(total_size))
    } else {
        format!(
            "准备发送 {} 个文件 ({})",
            ctx_poll.files.len(),
            format_human_size(total_size)
        )
    };
    let c_sum = cstr(&sum_text);
    unsafe { ws_share_set_file_summary(ui, c_sum.as_ptr()) };

    std::thread::spawn(move || {
        while !ctx_poll.stop_polling.load(Ordering::Relaxed)
            && !ctx_poll.transfer_started.load(Ordering::Relaxed)
        {
            let res = ipc_query_peers();
            push_update(&ctx_poll, UiUpdate::Devices(res));
            std::thread::sleep(Duration::from_millis(1200));
        }
    });
}

extern "C" fn cb_device_clicked(peer_id: *const c_char, user_data: *mut c_void) {
    let ctx = unsafe { Arc::from_raw(user_data as *const AppCtx) };
    let ctx_send = ctx.clone();
    std::mem::forget(ctx);

    if ctx_send.transfer_started.swap(true, Ordering::SeqCst) {
        return;
    }

    let target_peer = unsafe { CStr::from_ptr(peer_id) }
        .to_string_lossy()
        .into_owned();

    let ui = *ctx_send.ui.lock().unwrap();
    let title = cstr("正在建立加密连接并发送...");
    let sub = cstr("通过局域网 mTLS 直连传输");
    unsafe { ws_share_show_transfer(ui, title.as_ptr(), sub.as_ptr(), 0.0, 0, 0) };

    std::thread::spawn(move || {
        let Ok(mut stream) = UnixStream::connect(socket_path()) else {
            push_update(&ctx_send, UiUpdate::TransferError("无法连接到 寰宇工作台 后台".into()));
            return;
        };
        let req = ShareIpcCmd::SendFiles {
            peer_id: &target_peer,
            paths: &ctx_send.files,
            stream: true,
        };
        let line = serde_json::to_string(&req).unwrap() + "\n";
        if let Err(err) = stream.write_all(line.as_bytes()).and_then(|_| stream.flush()) {
            push_update(&ctx_send, UiUpdate::TransferError(err.to_string()));
            return;
        }

        let reader = BufReader::new(stream);
        for line_res in reader.lines() {
            let Ok(resp_line) = line_res else { break };
            let Ok(env) = serde_json::from_str::<DaemonStateEnvelope>(&resp_line) else {
                continue;
            };
            if let Some(rec) = env.record {
                *ctx_send.active_batch_id.lock().unwrap() = Some(rec.id.clone());
                let done = matches!(
                    rec.state,
                    TransferState::Completed | TransferState::Failed | TransferState::Cancelled
                );
                push_update(&ctx_send, UiUpdate::Transfer(rec));
                if done {
                    break;
                }
            } else if env.kind == "error" {
                push_update(
                    &ctx_send,
                    UiUpdate::TransferError(env.message.unwrap_or_else(|| "传输失败".into())),
                );
                break;
            }
        }
    });
}

extern "C" fn cb_cancel_clicked(user_data: *mut c_void) {
    let ctx = unsafe { Arc::from_raw(user_data as *const AppCtx) };
    let batch_id = ctx.active_batch_id.lock().unwrap().clone();
    std::mem::forget(ctx);

    if let Some(id) = batch_id {
        std::thread::spawn(move || {
            if let Ok(mut stream) = UnixStream::connect(socket_path()) {
                let req = ShareIpcCmd::CancelTransfer { batch_id: &id };
                let line = serde_json::to_string(&req).unwrap() + "\n";
                let _ = stream.write_all(line.as_bytes());
            }
        });
    }
}

extern "C" fn cb_idle_apply(user_data: *mut c_void) {
    let ctx = unsafe { &*(user_data as *const AppCtx) };
    let ui = *ctx.ui.lock().unwrap();
    if ui.is_null() {
        return;
    }
    let items: Vec<UiUpdate> = std::mem::take(&mut *ctx.pending.lock().unwrap());

    for item in items {
        match item {
            UiUpdate::Devices(Ok(peers)) => {
                if ctx.transfer_started.load(Ordering::Relaxed) {
                    continue;
                }
                unsafe { ws_share_clear_devices(ui) };
                if peers.is_empty() {
                    let msg = cstr("同一局域网内暂无在线的已配对手机\n请在 寰宇工作台 主界面确认手机已连接");
                    unsafe { ws_share_set_empty_message(ui, msg.as_ptr()) };
                } else {
                    for p in peers {
                        let id_c = cstr(&p.device_id);
                        let name_c = cstr(&p.device_name);
                        let sub = match (&p.ip, p.rtt_ms) {
                            (Some(ip), Some(rtt)) => format!("Connected · {ip} · {rtt} ms"),
                            (Some(ip), None) => format!("Connected · {ip}"),
                            _ => "Connected · LAN".to_string(),
                        };
                        let sub_c = cstr(&sub);
                        unsafe {
                            ws_share_add_device(
                                ui,
                                id_c.as_ptr(),
                                name_c.as_ptr(),
                                sub_c.as_ptr(),
                            )
                        };
                    }
                }
            }
            UiUpdate::Devices(Err(err)) => {
                if !ctx.transfer_started.load(Ordering::Relaxed) {
                    unsafe { ws_share_clear_devices(ui) };
                    let err_c = cstr(&err);
                    unsafe { ws_share_set_empty_message(ui, err_c.as_ptr()) };
                }
            }
            UiUpdate::Transfer(rec) => {
                let frac = if rec.total_bytes > 0 {
                    (rec.bytes_transferred as f64 / rec.total_bytes as f64).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                match rec.state {
                    TransferState::Transferring => {
                        let t = cstr(&format!("正在发送至 {}", rec.peer_name));
                        let s = cstr(&format!(
                            "{} / {}",
                            format_human_size(rec.bytes_transferred),
                            format_human_size(rec.total_bytes)
                        ));
                        unsafe { ws_share_show_transfer(ui, t.as_ptr(), s.as_ptr(), frac, 0, 0) };
                    }
                    TransferState::Completed => {
                        let t = cstr(&format!("已发送至 {}", rec.peer_name));
                        let s = cstr(&format!(
                            "{} ({})",
                            rec.summary,
                            format_human_size(rec.total_bytes)
                        ));
                        unsafe { ws_share_show_transfer(ui, t.as_ptr(), s.as_ptr(), 1.0, 1, 0) };
                    }
                    TransferState::Cancelled => {
                        let t = cstr("传输已取消");
                        let s = cstr(&rec.summary);
                        unsafe { ws_share_show_transfer(ui, t.as_ptr(), s.as_ptr(), frac, 1, 1) };
                    }
                    TransferState::Failed => {
                        let t = cstr("传输失败");
                        let s = cstr(rec.error_message.as_deref().unwrap_or("未知错误"));
                        unsafe { ws_share_show_transfer(ui, t.as_ptr(), s.as_ptr(), frac, 1, 1) };
                    }
                }
            }
            UiUpdate::TransferError(msg) => {
                let t = cstr("传输失败");
                let s = cstr(&msg);
                unsafe { ws_share_show_transfer(ui, t.as_ptr(), s.as_ptr(), 0.0, 1, 1) };
            }
        }
    }
}

fn main() {
    let files: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect();

    if files.is_empty() {
        eprintln!("Usage: workspace-share <file1> [file2 ...]");
        std::process::exit(1);
    }

    let ctx = Arc::new(AppCtx {
        ui: Mutex::new(std::ptr::null_mut()),
        files,
        pending: Mutex::new(Vec::new()),
        active_batch_id: Mutex::new(None),
        transfer_started: AtomicBool::new(false),
        stop_polling: AtomicBool::new(false),
    });

    let cbs = WsShareCallbacks {
        on_ready: cb_ready,
        on_device_clicked: cb_device_clicked,
        on_cancel_clicked: cb_cancel_clicked,
    };

    let raw = Arc::into_raw(ctx.clone()) as *mut c_void;
    let ui_ptr = unsafe { ws_share_ui_new(cbs, raw) };
    *ctx.ui.lock().unwrap() = ui_ptr;

    let rc = unsafe { ws_share_ui_run(ui_ptr) };
    ctx.stop_polling.store(true, Ordering::Relaxed);
    std::process::exit(rc);
}
