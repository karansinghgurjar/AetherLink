#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
const SUPPRESSION_WINDOW: Duration = Duration::from_secs(3);
#[cfg(windows)]
static NEXT_SYNC_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(windows)]
#[derive(Debug, Default)]
pub struct ClipboardSyncState {
    last_applied_hash: Option<String>,
    last_applied_sync_id: Option<String>,
    last_applied_at: Option<Instant>,
    last_sent_hash: Option<String>,
}

#[cfg(windows)]
pub fn hash_text(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[cfg(windows)]
pub fn next_sync_id(source: &str) -> String {
    format!("{}-{}", source, NEXT_SYNC_ID.fetch_add(1, Ordering::Relaxed))
}

#[cfg(windows)]
impl ClipboardSyncState {
    pub fn should_suppress_incoming(&self, text: &str, sync_id: Option<&str>) -> bool {
        let hash = hash_text(text);
        if let (Some(expected), Some(actual)) = (self.last_applied_sync_id.as_deref(), sync_id) {
            if expected == actual {
                return true;
            }
        }
        if let (Some(last_hash), Some(last_applied_at)) = (&self.last_applied_hash, self.last_applied_at) {
            if last_hash == &hash && last_applied_at.elapsed() <= SUPPRESSION_WINDOW {
                return true;
            }
        }
        false
    }

    pub fn mark_applied(&mut self, text: &str, sync_id: Option<&str>) {
        self.last_applied_hash = Some(hash_text(text));
        self.last_applied_sync_id = sync_id.map(ToOwned::to_owned);
        self.last_applied_at = Some(Instant::now());
    }

    pub fn prepare_host_emit(&mut self, text: &str) -> Option<String> {
        let hash = hash_text(text);
        if self.last_sent_hash.as_deref() == Some(hash.as_str()) {
            return None;
        }
        self.last_sent_hash = Some(hash);
        Some(next_sync_id("host"))
    }
}
