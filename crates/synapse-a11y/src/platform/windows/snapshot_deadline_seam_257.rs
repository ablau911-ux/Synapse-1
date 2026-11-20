//! #257 test seam: lengthen the snapshot walk deadline for ONE window, so a
//! slow synthetic UIA fixture reaches node-budget limits deterministically.
//!
//! Snapshots run on the shared UIA worker thread, so a test-thread
//! thread_local would never reach them. The override is a process-wide static
//! keyed by the snapshot root HWND: snapshots of other windows (parallel
//! tests) keep `SNAPSHOT_DEADLINE`. Only one override is active at a time; the
//! RAII guard holds the owner lock and clears the override on drop.
//!
//! A long walk occupies the shared UIA worker for seconds, so the guard also
//! holds the exclusive worker test gate: other tests' UIA jobs wait at the gate
//! (untimed) instead of hitting the worker queue/reply deadlines.

use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use super::super::common::worker_test_gate::{self, ExclusiveGate};

static OVERRIDE: Mutex<Option<(i64, Duration)>> = Mutex::new(None);
static OWNER: Mutex<()> = Mutex::new(());

/// While alive, snapshots rooted at the given window use the given deadline.
pub(super) struct SnapshotDeadlineOverride {
    _owner: MutexGuard<'static, ()>,
    _worker: ExclusiveGate,
}

impl SnapshotDeadlineOverride {
    pub(super) fn for_window(hwnd: i64, deadline: Duration) -> Self {
        let worker = worker_test_gate::exclusive();
        let owner = OWNER.lock().unwrap_or_else(PoisonError::into_inner);
        *OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner) = Some((hwnd, deadline));
        Self {
            _owner: owner,
            _worker: worker,
        }
    }
}

impl Drop for SnapshotDeadlineOverride {
    fn drop(&mut self) {
        *OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// The active override for a snapshot rooted at `root_hwnd`, if any.
pub(super) fn override_for(root_hwnd: i64) -> Option<Duration> {
    let current = *OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner);
    current
        .filter(|(hwnd, _)| *hwnd == root_hwnd)
        .map(|(_, deadline)| deadline)
}
