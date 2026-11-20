//! Warn-once bookkeeping for the orphan lease sweep (#180).
//!
//! The sweep runs every 250 ms. A single undecodable or mismatched lease row
//! used to log `MCP_SESSION_LEASE_ORPHAN_SWEEP_ROW_SKIPPED` on every pass
//! (~345k lines per day per row). [`SkippedRowWarnings`] remembers which corrupt
//! keys were already reported so each is logged once, while the row itself is
//! left in place for repair (the sweep never deletes data it cannot decode).
//!
//! The memory is bounded and self-healing: after every sweep the remembered set
//! is pruned to the keys that were corrupt in that sweep, so a repaired or
//! deleted row is forgotten and a row that becomes corrupt again warns again.

use std::collections::HashSet;
use std::sync::Mutex;

/// Upper bound on remembered keys; further new keys are counted, not stored.
pub(super) const MAX_REMEMBERED_KEYS: usize = 1024;

#[derive(Debug, Default)]
pub(super) struct SkippedRowWarnings {
    warned: Mutex<HashSet<Vec<u8>>>,
}

/// Collects the corrupt keys seen during one sweep.
#[derive(Debug, Default)]
pub(super) struct SweepSkips {
    seen: HashSet<Vec<u8>>,
}

impl SkippedRowWarnings {
    pub(super) fn startup_lease_rows() -> &'static Self {
        static STARTUP: std::sync::OnceLock<SkippedRowWarnings> = std::sync::OnceLock::new();
        STARTUP.get_or_init(Self::default)
    }

    /// Process-wide instance used by the daemon sweep.
    pub(super) fn global() -> &'static Self {
        static GLOBAL: std::sync::OnceLock<SkippedRowWarnings> = std::sync::OnceLock::new();
        GLOBAL.get_or_init(Self::default)
    }

    /// Process-wide instance for the persisted CDP target-owner row scans
    /// (#216). Separate from [`Self::global`]: every scan prunes the remembered
    /// set to the keys it saw, so scans over different row families must not
    /// share one memory.
    pub(super) fn cdp_owner_rows() -> &'static Self {
        static CDP_OWNER_ROWS: std::sync::OnceLock<SkippedRowWarnings> = std::sync::OnceLock::new();
        CDP_OWNER_ROWS.get_or_init(Self::default)
    }

    /// Process-wide instance for the persisted session-target row scan (#216).
    pub(super) fn session_target_rows() -> &'static Self {
        static SESSION_TARGET_ROWS: std::sync::OnceLock<SkippedRowWarnings> =
            std::sync::OnceLock::new();
        SESSION_TARGET_ROWS.get_or_init(Self::default)
    }

    /// Records `key` as corrupt in this sweep. Returns `true` exactly when the
    /// caller should log a warning: the first sighting since the key was last
    /// healthy. When the remembered set is full, further new keys stay quiet
    /// rather than risk a per-tick flood.
    pub(super) fn should_warn(&self, sweep: &mut SweepSkips, key: &[u8]) -> bool {
        sweep.seen.insert(key.to_vec());
        let Ok(mut warned) = self.warned.lock() else {
            // A poisoned set must not turn into a log flood; stay quiet.
            return false;
        };
        if warned.contains(key) {
            return false;
        }
        if warned.len() >= MAX_REMEMBERED_KEYS {
            return false;
        }
        warned.insert(key.to_vec())
    }

    /// Forgets every key that was not corrupt in the finished sweep.
    pub(super) fn finish_sweep(&self, sweep: SweepSkips) {
        if let Ok(mut warned) = self.warned.lock() {
            warned.retain(|key| sweep.seen.contains(key));
        }
    }

    /// Whether `key` is currently remembered as already warned (tests).
    #[cfg(test)]
    pub(super) fn is_remembered(&self, key: &[u8]) -> bool {
        self.warned.lock().is_ok_and(|warned| warned.contains(key))
    }

    /// Number of remembered keys (tests).
    #[cfg(test)]
    pub(super) fn remembered(&self) -> usize {
        self.warned.lock().map_or(0, |warned| warned.len())
    }
}
