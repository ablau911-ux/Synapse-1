//! Test-only gate around the process-wide UIA worker (T1 raid, #61/#32).
//!
//! The #61 worker-admission tests deliberately hang a job, retire the shared
//! worker and saturate `LIVE_UIA_WORKER_THREADS`. Running them while another
//! test's UIA job is queued on the same worker made that unrelated job fail
//! with `phase=worker_unavailable` or `phase=worker_limit`. Every UIA job in
//! the unit-test binary takes the gate shared; a test that rewires worker state
//! takes it exclusively. The exclusive holder's own UIA calls pass through.

use std::cell::Cell;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

static GATE: RwLock<()> = RwLock::new(());

thread_local! {
    static EXCLUSIVE_OWNER: Cell<bool> = const { Cell::new(false) };
}

/// Held for one UIA job; `None` when this thread owns the exclusive gate.
pub(super) struct SharedGate {
    _guard: Option<RwLockReadGuard<'static, ()>>,
}

pub(super) fn shared() -> SharedGate {
    if EXCLUSIVE_OWNER.with(Cell::get) {
        return SharedGate { _guard: None };
    }
    SharedGate {
        _guard: Some(GATE.read().unwrap_or_else(PoisonError::into_inner)),
    }
}

/// Held for a whole test that rewires the shared worker state.
pub(in crate::platform::windows) struct ExclusiveGate {
    _guard: RwLockWriteGuard<'static, ()>,
}

pub(in crate::platform::windows) fn exclusive() -> ExclusiveGate {
    let guard = GATE.write().unwrap_or_else(PoisonError::into_inner);
    EXCLUSIVE_OWNER.with(|owner| owner.set(true));
    ExclusiveGate { _guard: guard }
}

impl ExclusiveGate {
    /// Lets a helper thread of the exclusive test (e.g. a `thread::scope`
    /// thread that occupies the worker) pass the gate while this is held.
    pub(super) fn admit_current_thread(&self) -> AdmittedThread<'_> {
        EXCLUSIVE_OWNER.with(|owner| owner.set(true));
        AdmittedThread { _gate: self }
    }
}

impl Drop for ExclusiveGate {
    fn drop(&mut self) {
        EXCLUSIVE_OWNER.with(|owner| owner.set(false));
    }
}

/// A helper thread's pass through the exclusive gate; cleared when dropped.
pub(super) struct AdmittedThread<'a> {
    _gate: &'a ExclusiveGate,
}

impl Drop for AdmittedThread<'_> {
    fn drop(&mut self) {
        EXCLUSIVE_OWNER.with(|owner| owner.set(false));
    }
}
