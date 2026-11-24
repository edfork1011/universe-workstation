// SPDX-License-Identifier: MIT
//! `me-edfork-workspace` entrypoint: single-instance GUI, `--silent` tray mode, and CLI control.

mod clipboard;
mod config;
mod daemon;
mod dedup;
mod notify;
mod tray;
mod ui;
mod watcher;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;
use workspace_core::mtls_http::PeerCallbacks;
use workspace_core::protocol::PairState;

use crate::daemon::{IpcRequest, IpcResponse, StartedDaemon, UiEvent, open_folder_xdg, send_ipc_once};
use crate::tray::TrayAction;
use crate::ui::{UiCommand, create_gtk_controller, handle_ui_command, push_event_to_gtk, run_gtk_main_loop};

#[derive(Debug, Parser)]
#[command(
    name = "me-edfork-workspace",
    version = workspace_core::version::VERSION_NAME,
    long_version = workspace_core::version::VERSION_FULL_LABEL,
    about = "寰宇工作台 · 局域网安全互联 Linux 客户端"
)]
struct Cli {
    /// Start silently in background with persistent system tray icon (no window shown).
    #[arg(long)]
    silent: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Show daemon status and connected peers.
    Status,
    /// List paired and nearby discovered LAN devices.
    Peers,
    /// Initiate 6-digit SAS pairing with a discovered peer.
    Pair { device_id: String },
    /// Approve an active SAS pairing session.
    Approve { session_id: String },
    /// Reject an active SAS pairing session.
    Reject { session_id: String },
    /// Revoke trust / unpair a device immediately.
    Unpair { device_id: String },
    /// Send local files over LAN to a paired device.
    Send {
        peer_id: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Request already-running daemon to present main window.
    Show,
    /// Gracefully stop the running daemon.
    Quit,
}

fn current_activation_token() -> Option<String> {
    for key in ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return rt.block_on(run_cli_subcommand(cmd));
    }

    // Single-instance check before spawning daemon + GTK main loop.
    let probe_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let already_running = probe_rt.block_on(async {
        let req = if cli.silent {
            IpcRequest::GetState
        } else {
            IpcRequest::ShowWindow {
                activation_token: current_activation_token(),
            }
        };
        send_ipc_once(&req).await.is_ok()
    });
    drop(probe_rt);

    if already_running {
        return Ok(());
    }

    let (tray_tx, mut tray_rx) = mpsc::unbounded_channel::<TrayAction>();
    let (ui_cmd_tx, mut ui_cmd_rx) = mpsc::unbounded_channel::<UiCommand>();

    let gtk_ctx = create_gtk_controller(cli.silent, ui_cmd_tx);
    let gtk_ctx_bg = gtk_ctx.clone();

    let bg_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let daemon = bg_rt.block_on(StartedDaemon::start(None, tray_tx))?;
    let runtime = daemon.runtime.clone();
    let runtime_for_gtk = runtime.clone();

    // Forward daemon UI events -> GTK idle loop and Tray/GTK actions -> daemon handlers.
    bg_rt.spawn(async move {
        let mut ui_sub = runtime.subscribe_ui();
        loop {
            tokio::select! {
                ev = ui_sub.recv() => {
                    match ev {
                        Ok(UiEvent::Quit) => {
                            push_event_to_gtk(&gtk_ctx_bg, UiEvent::Quit);
                            break;
                        }
                        Ok(other) => {
                            push_event_to_gtk(&gtk_ctx_bg, other);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            push_event_to_gtk(&gtk_ctx_bg, UiEvent::Snapshot(Box::new(runtime.snapshot())));
                        }
                        Err(_) => break,
                    }
                }
                Some(action) = tray_rx.recv() => {
                    match action {
                        TrayAction::ShowWindow => runtime.request_show_window(None),
                        TrayAction::OpenReceiveDir => {
                            let dir = runtime.receive_dir();
                            let _ = std::fs::create_dir_all(&dir);
                            open_folder_xdg(&dir);
                        }
                        TrayAction::ToggleSyncPaused => {
                            let cur = runtime.config.read().expect("config lock poisoned").sync_paused;
                            let _ = runtime.set_sync_paused(!cur);
                        }
                        TrayAction::Quit => {
                            runtime.request_quit();
                        }
                    }
                }
                Some(cmd) = ui_cmd_rx.recv() => {
                    handle_ui_command(&runtime, cmd).await;
                }
            }
        }
    });

    // Run GTK4 main loop on main thread (blocks until Quit).
    let exit_code = run_gtk_main_loop(&gtk_ctx, runtime_for_gtk);
    daemon.runtime.request_quit();
    bg_rt.block_on(daemon.wait_until_shutdown());
    if exit_code != 0 {
        anyhow::bail!("GTK main loop exited with status {exit_code}");
    }
    Ok(())
}

async fn run_cli_subcommand(cmd: Commands) -> anyhow::Result<()> {
    let resp = match cmd {
        Commands::Status => send_ipc_once(&IpcRequest::GetState)
            .await
            .map_err(anyhow::Error::msg)?,
        Commands::Peers => send_ipc_once(&IpcRequest::GetState)
            .await
            .map_err(anyhow::Error::msg)?,
        Commands::Pair { device_id } => send_ipc_once(&IpcRequest::StartPair { device_id })
            .await
            .map_err(anyhow::Error::msg)?,
        Commands::Approve { session_id } => {
            send_ipc_once(&IpcRequest::ApprovePair { session_id })
                .await
                .map_err(anyhow::Error::msg)?
        }
        Commands::Reject { session_id } => send_ipc_once(&IpcRequest::RejectPair { session_id })
            .await
            .map_err(anyhow::Error::msg)?,
        Commands::Unpair { device_id } => send_ipc_once(&IpcRequest::Unpair { device_id })
            .await
            .map_err(anyhow::Error::msg)?,
        Commands::Send { peer_id, files } => send_ipc_once(&IpcRequest::SendFiles {
            peer_id,
            paths: files,
            stream: false,
        })
        .await
        .map_err(anyhow::Error::msg)?,
        Commands::Show => send_ipc_once(&IpcRequest::ShowWindow {
            activation_token: current_activation_token(),
        })
        .await
        .map_err(anyhow::Error::msg)?,
        Commands::Quit => send_ipc_once(&IpcRequest::Quit)
            .await
            .map_err(anyhow::Error::msg)?,
    };

    match resp {
        IpcResponse::Ok => println!("OK"),
        IpcResponse::State { snapshot } => {
            println!("本机：{} (ID={}, 指纹={})", snapshot.device_name, snapshot.device_id, snapshot.fingerprint);
            println!("已配对设备 ({}):", snapshot.paired_peers.len());
            for p in snapshot.paired_peers {
                println!(
                    "  - {} [{}] connected={} ip={:?} rtt={:?}ms",
                    p.device_name, p.device_id, p.connected, p.ip, p.rtt_ms
                );
            }
            println!("局域网发现未配对设备 ({}):", snapshot.discovered_peers.len());
            for d in snapshot.discovered_peers {
                println!(
                    "  - {} [{}] {}:{} fp={}",
                    d.device_name, d.device_id, d.ip, d.tls_port, d.fingerprint
                );
            }
            if !snapshot.pending_pairs.is_empty() {
                println!("待验证配对会话:");
                for s in snapshot.pending_pairs {
                    println!(
                        "  - session={} peer={} sas='{}' state={:?}",
                        s.session_id, s.peer_name, s.sas_code, s.state
                    );
                }
            }
        }
        IpcResponse::PairSession { session } => {
            println!(
                "配对会话 {} | 对端: {} | 6位验证码: {} | 状态: {:?}",
                session.session_id, session.peer_name, session.sas_code, session.state
            );
            if session.state == PairState::Pending && session.is_initiator {
                println!("核对手机端验证码一致后执行: me-edfork-workspace approve {}", session.session_id);
            }
        }
        IpcResponse::TransferFinished { record } | IpcResponse::TransferProgress { record } => {
            println!(
                "传输 {}: {} ({} / {} bytes) state={:?}",
                record.id, record.summary, record.bytes_transferred, record.total_bytes, record.state
            );
        }
        IpcResponse::Error { message } => anyhow::bail!("{message}"),
    }
    Ok(())
}
