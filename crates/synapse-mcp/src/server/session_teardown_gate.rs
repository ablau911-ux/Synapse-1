//! Per-session teardown serialization (#213).
//!
//! Teardown can take tens of seconds (process-tree waits, CDP closes). Without
//! mutual exclusion the 30 s failed-cleanup retry, the stale sweep, a DELETE
//! and `agent_kill`/`adopt` could all run a teardown of the same session at
//! once: both read the same owner rows, both close the same tab, one fails
//! "target not present", and the loser reports a spurious failure.
//!
//! [`SessionTeardownGate`] hands out one [`SessionTeardownPermit`] per session
//! id at a time. A second teardown waits for the first; the sweep and retry
//! paths ask [`SessionTeardownGate::is_in_progress`] and skip such sessions, so
//! "teardown still running" is never confused with "teardown failed".

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

struct GateEntry {
    lock: Arc<AsyncMutex<()>>,
    /// Permits held or queued for this session id.
    users: usize,
}

#[derive(Default)]
pub(super) struct SessionTeardownGate {
    entries: Mutex<BTreeMap<String, GateEntry>>,
}

/// Held for the duration of one session teardown; releases on drop, including
/// when the teardown future is cancelled while running or while queued.
pub(super) struct SessionTeardownPermit<'gate> {
    gate: &'gate SessionTeardownGate,
    session_id: String,
    guard: Option<OwnedMutexGuard<()>>,
}

impl SessionTeardownGate {
    /// Process-wide gate: `SessionLifecycleState` is rebuilt per call, so the
    /// exclusion state cannot live in it.
    pub(super) fn global() -> &'static Self {
        static GLOBAL: OnceLock<SessionTeardownGate> = OnceLock::new();
        GLOBAL.get_or_init(Self::default)
    }

    /// Waits until no other teardown of `session_id` is running, then claims it.
    pub(super) async fn begin(&self, session_id: &str) -> SessionTeardownPermit<'_> {
        let lock = {
            let mut entries = self.locked_entries();
            let entry = entries
                .entry(session_id.to_owned())
                .or_insert_with(|| GateEntry {
                    lock: Arc::new(AsyncMutex::new(())),
                    users: 0,
                });
            entry.users += 1;
            Arc::clone(&entry.lock)
        };
        // Registered: from here the permit's Drop undoes the registration even
        // if this future is cancelled while queued behind another teardown.
        let mut permit = SessionTeardownPermit {
            gate: self,
            session_id: session_id.to_owned(),
            guard: None,
        };
        permit.guard = Some(lock.lock_owned().await);
        permit
    }

    /// Whether a teardown of `session_id` is running or queued.
    pub(super) fn is_in_progress(&self, session_id: &str) -> bool {
        self.locked_entries().contains_key(session_id)
    }

    fn locked_entries(&self) -> MutexGuard<'_, BTreeMap<String, GateEntry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for SessionTeardownPermit<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        let mut entries = self.gate.locked_entries();
        let last_user = entries.get_mut(&self.session_id).is_some_and(|entry| {
            entry.users = entry.users.saturating_sub(1);
            entry.users == 0
        });
        if last_user {
            entries.remove(&self.session_id);
        }
    }
}
