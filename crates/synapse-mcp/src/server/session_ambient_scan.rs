//! Short-TTL cache for the ambient agent process/window scan behind
//! `session list` (#209).
//!
//! The scan reads every process's command line (and optionally cwd) from its
//! PEB and enumerates top-level windows. Orchestrators and the dashboard tray
//! poll `session list`, so one scan is shared for a short window instead of
//! being repeated for every call.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

struct Entry<T> {
    taken_at: Instant,
    with_cwd: bool,
    value: Arc<T>,
}

pub(super) struct ScanCache<T> {
    ttl: Duration,
    slot: Mutex<Option<Entry<T>>>,
}

impl<T> ScanCache<T> {
    pub(super) const fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            slot: Mutex::new(None),
        }
    }

    /// Returns the cached scan when it is younger than the TTL and was taken
    /// with cwd collection if `need_cwd` is set; otherwise runs `scan` while
    /// holding the slot so concurrent callers share one scan.
    pub(super) fn get(&self, need_cwd: bool, scan: impl FnOnce() -> T) -> Arc<T> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = slot.as_ref()
            && entry.taken_at.elapsed() < self.ttl
            && (entry.with_cwd || !need_cwd)
        {
            return Arc::clone(&entry.value);
        }
        let value = Arc::new(scan());
        *slot = Some(Entry {
            taken_at: Instant::now(),
            with_cwd: need_cwd,
            value: Arc::clone(&value),
        });
        value
    }

    /// Test seam: drops the cached scan so the next `get` scans fresh.
    #[cfg(test)]
    pub(super) fn reset(&self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
#[path = "tests/session_ambient_scan_209.rs"]
mod session_ambient_scan_209;
