//! Loop-prevention deduplication tracker for synced clipboard payloads.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_RECENT_HASHES: usize = 16;
const DEDUP_TTL: Duration = Duration::from_secs(12);

#[derive(Debug)]
struct Entry {
    checksum: String,
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct DedupTracker {
    recent_remote: VecDeque<Entry>,
    last_written_checksum: Option<String>,
    suppress_watch_events: usize,
}

impl DedupTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn prune(&mut self, now: Instant) {
        while let Some(front) = self.recent_remote.front() {
            if front.expires_at <= now {
                self.recent_remote.pop_front();
            } else {
                break;
            }
        }
    }

    /// Record a payload received from a remote peer and written to the local clipboard.
    pub fn mark_remote_applied(&mut self, checksum: &str) {
        let now = Instant::now();
        self.prune(now);
        self.last_written_checksum = Some(checksum.to_string());
        self.suppress_watch_events = self.suppress_watch_events.saturating_add(1).min(4);
        self.recent_remote.push_back(Entry {
            checksum: checksum.to_string(),
            expires_at: now + DEDUP_TTL,
        });
        while self.recent_remote.len() > MAX_RECENT_HASHES {
            self.recent_remote.pop_front();
        }
    }

    /// Returns `true` if `checksum` matches the payload just applied from a peer (avoid duplicate apply).
    pub fn is_duplicate_remote(&mut self, checksum: &str) -> bool {
        let now = Instant::now();
        self.prune(now);
        self.last_written_checksum.as_deref() == Some(checksum)
            || self.recent_remote.iter().any(|e| e.checksum == checksum)
    }

    /// Returns `true` if a watcher capture event should be ignored because it came from our own `wl-copy`.
    pub fn should_ignore_local_capture(&mut self, checksum: &str) -> bool {
        let now = Instant::now();
        self.prune(now);
        if self.suppress_watch_events > 0
            && self.recent_remote.iter().any(|e| e.checksum == checksum)
        {
            self.suppress_watch_events -= 1;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_prevents_self_echo_loop_and_duplicate_apply() {
        let mut tracker = DedupTracker::new();
        tracker.mark_remote_applied("hash_abc");
        assert!(tracker.is_duplicate_remote("hash_abc"));
        assert!(tracker.should_ignore_local_capture("hash_abc"));
        assert!(!tracker.should_ignore_local_capture("hash_other"));
    }
}
