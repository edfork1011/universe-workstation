//! Supervises `wl-paste --watch` so latest local clipboard snapshots stay fresh without blocking pulls.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use tracing::{debug, warn};
use workspace_core::checksum::compute_blake3_hex;
use workspace_core::clipboard_model::{ClipboardMessage, ValidatedPayload};

use crate::clipboard::read_wayland_clipboard;
use crate::dedup::DedupTracker;

pub type SharedDedup = Arc<tokio::sync::Mutex<DedupTracker>>;
pub type LatestSnapshot = Arc<tokio::sync::RwLock<Option<ClipboardMessage>>>;

/// Spawn `wl-paste --watch` supervisor updating `latest_snapshot` on local Wayland copy events.
pub fn spawn_clipboard_watcher(
    origin: String,
    dedup: SharedDedup,
    latest_snapshot: LatestSnapshot,
    mut shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut sequence: u64 = 1;
        let mut backoff = Duration::from_secs(1);

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let spawned = Command::new("wl-paste")
                .arg("--watch")
                .arg("echo")
                .arg("CLIPBOARD_EVENT")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn();

            let mut child = match spawned {
                Ok(c) => {
                    backoff = Duration::from_secs(1);
                    c
                }
                Err(err) => {
                    warn!("wl-paste --watch unavailable ({err}); retrying in {backoff:?}");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = shutdown_rx.changed() => break,
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(15));
                    continue;
                }
            };

            let Some(stdout) = child.stdout.take() else {
                break;
            };
            let mut lines = BufReader::new(stdout).lines();

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            let _ = child.kill().await;
                            return;
                        }
                    }
                    line_res = lines.next_line() => {
                        match line_res {
                            Ok(Some(_)) => {
                                sequence = sequence.wrapping_add(1);
                                match read_wayland_clipboard(&origin, sequence).await {
                                    Ok(msg) => {
                                        let mut tracker = dedup.lock().await;
                                        if tracker.should_ignore_local_capture(&msg.checksum) {
                                            debug!("clipboard watcher skipped self-written payload");
                                        } else {
                                            drop(tracker);
                                            *latest_snapshot.write().await = Some(msg);
                                        }
                                    }
                                    Err(err) => {
                                        debug!("clipboard capture ignored: {err}");
                                    }
                                }
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                }
            }

            let _ = child.wait().await;
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = shutdown_rx.changed() => break,
            }
            backoff = (backoff * 2).min(Duration::from_secs(10));
        }
    })
}

/// Apply validated remote clipboard payload to Wayland while recording dedup & snapshot state.
pub async fn apply_remote_clipboard(
    msg: ClipboardMessage,
    payload: &ValidatedPayload,
    dedup: &SharedDedup,
    latest_snapshot: &LatestSnapshot,
) -> Result<(bool, bool), String> {
    let checksum = match payload {
        ValidatedPayload::Text { text, .. } => compute_blake3_hex(text.as_bytes()),
        ValidatedPayload::Image { bytes, .. } => compute_blake3_hex(bytes),
    };

    {
        let mut tracker = dedup.lock().await;
        if tracker.is_duplicate_remote(&checksum) {
            return Ok((false, true));
        }
        tracker.mark_remote_applied(&checksum);
    }

    crate::clipboard::write_wayland_clipboard(payload)
        .await
        .map_err(|e| e.to_string())?;
    *latest_snapshot.write().await = Some(msg);
    Ok((true, false))
}
