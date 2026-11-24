// SPDX-License-Identifier: MIT
//! `workspaced` entrypoint: background KernelSU root daemon & localhost CLI control.

mod bridge;
mod config;
mod daemon;
mod local_ipc;

use std::path::PathBuf;

use bytes::Bytes;
use clap::{Parser, Subcommand};
use http::Request;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tracing_subscriber::EnvFilter;
use workspace_core::limits::DEFAULT_ANDROID_LOCAL_PORT;

use crate::daemon::StartedAndroidDaemon;

#[derive(Debug, Parser)]
#[command(
    name = "workspaced",
    version = workspace_core::version::VERSION_NAME,
    long_version = workspace_core::version::VERSION_FULL_LABEL,
    about = "寰宇工作台 Android KernelSU 守护进程与控制 CLI"
)]
struct Cli {
    #[arg(long)]
    state_dir: Option<PathBuf>,

    #[arg(long, default_value_t = DEFAULT_ANDROID_LOCAL_PORT)]
    local_port: u16,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run background daemon (`workspaced daemon`).
    Daemon,
    /// Print daemon status from 127.0.0.1 IPC.
    Status,
    /// Push phone clipboard to paired Linux PC.
    Push { peer_id: Option<String> },
    /// Pull Linux PC clipboard to phone.
    Pull { peer_id: Option<String> },
    /// Start SAS pairing with a discovered PC.
    Pair { device_id: String },
    /// Approve an active SAS pairing session.
    Approve { session_id: String },
    /// Unpair a paired PC.
    Unpair { device_id: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Daemon) {
        Commands::Daemon => {
            let daemon = StartedAndroidDaemon::start(cli.state_dir).await?;
            let rt_shutdown = daemon.runtime.clone();
            tokio::select! {
                _ = daemon.wait_until_shutdown() => {}
                _ = tokio::signal::ctrl_c() => {
                    rt_shutdown.request_shutdown();
                }
            }
        }
        Commands::Status => {
            let body = local_http_call(cli.local_port, http::Method::GET, "/v1/local/status", None).await?;
            println!("{body}");
        }
        Commands::Push { peer_id } => {
            let json = serde_json::json!({ "peer_id": peer_id }).to_string();
            let body = local_http_call(
                cli.local_port,
                http::Method::POST,
                "/v1/local/clipboard/push",
                Some(json),
            )
            .await?;
            println!("{body}");
        }
        Commands::Pull { peer_id } => {
            let json = serde_json::json!({ "peer_id": peer_id }).to_string();
            let body = local_http_call(
                cli.local_port,
                http::Method::POST,
                "/v1/local/clipboard/pull",
                Some(json),
            )
            .await?;
            println!("{body}");
        }
        Commands::Pair { device_id } => {
            let json = serde_json::json!({ "device_id": device_id }).to_string();
            let body = local_http_call(
                cli.local_port,
                http::Method::POST,
                "/v1/local/pair/start",
                Some(json),
            )
            .await?;
            println!("{body}");
        }
        Commands::Approve { session_id } => {
            let json = serde_json::json!({ "session_id": session_id }).to_string();
            let body = local_http_call(
                cli.local_port,
                http::Method::POST,
                "/v1/local/pair/approve",
                Some(json),
            )
            .await?;
            println!("{body}");
        }
        Commands::Unpair { device_id } => {
            let json = serde_json::json!({ "device_id": device_id }).to_string();
            let body = local_http_call(
                cli.local_port,
                http::Method::POST,
                "/v1/local/unpair",
                Some(json),
            )
            .await?;
            println!("{body}");
        }
    }
    Ok(())
}

async fn local_http_call(
    port: u16,
    method: http::Method,
    path: &str,
    json_body: Option<String>,
) -> anyhow::Result<String> {
    let stream = TcpStream::connect(("127.0.0.1", port)).await?;
    let (mut sender, conn) = http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let bytes = Bytes::from(json_body.unwrap_or_default());
    let len = bytes.len().to_string();
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(http::header::HOST, "127.0.0.1")
        .header(http::header::CONTENT_LENGTH, len);
    if !bytes.is_empty() {
        builder = builder.header(http::header::CONTENT_TYPE, "application/json");
    }
    let req = builder.body(Full::new(bytes))?;
    let resp = sender.send_request(req).await?;
    let status = resp.status();
    let body = String::from_utf8_lossy(&resp.into_body().collect().await?.to_bytes()).to_string();
    if !status.is_success() {
        anyhow::bail!("HTTP {status}: {body}");
    }
    Ok(body)
}
