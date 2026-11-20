//! DEBUG-BUILD-ONLY fault hook for session process teardown (#75).
//!
//! Reviewers cannot make a real session-owned process termination fail
//! headlessly (job-object kill succeeds), so the "failed cleanup keeps ownership
//! until a retry verifies exit" rule had no live proof. With
//! `SYNAPSE_DEBUG_FAULT_CLEANUP_FAIL_PID=<pid>[:<count>]` a debug daemon makes
//! the teardown of the owned process tree containing `<pid>` report that pid as
//! a survivor, `<count>` times (default 1). The session keeps ownership of the
//! resource (`retained_processes`), the teardown reports `failed`, and the next
//! retry observes the real process state and either clears ownership (exit
//! verified) or retains it again.
//!
//! This module is declared under `#[cfg(debug_assertions)]` in
//! `session_lifecycle.rs`, as is its only call site, so it does not exist in
//! release builds at all - the environment variable is never read there.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

/// Environment variable naming the pid whose teardown is forced to fail.
pub(super) const FAULT_ENV: &str = "SYNAPSE_DEBUG_FAULT_CLEANUP_FAIL_PID";

/// Which pid to fail and how many more times.
#[derive(Debug)]
pub(super) struct FaultPlan {
    pid: u32,
    remaining_failures: AtomicU32,
}

impl FaultPlan {
    /// Parses `<pid>` or `<pid>:<count>`; `None` for anything invalid
    /// (pid 0 and count 0 included).
    pub(super) fn parse(spec: &str) -> Option<Self> {
        let (pid_raw, count_raw) = match spec.trim().split_once(':') {
            Some((pid, count)) => (pid, Some(count)),
            None => (spec.trim(), None),
        };
        let pid = pid_raw.trim().parse::<u32>().ok().filter(|pid| *pid != 0)?;
        let count = match count_raw {
            Some(raw) => raw.trim().parse::<u32>().ok().filter(|count| *count != 0)?,
            None => 1,
        };
        Some(Self {
            pid,
            remaining_failures: AtomicU32::new(count),
        })
    }

    /// If the planned pid is part of `process_ids` and failures remain, spends
    /// one and returns the pid to report as a survivor.
    pub(super) fn consume_for(&self, process_ids: &[u32]) -> Option<u32> {
        if !process_ids.contains(&self.pid) {
            return None;
        }
        self.remaining_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .ok()
            .map(|_previous| self.pid)
    }
}

fn global_plan() -> Option<&'static FaultPlan> {
    static PLAN: OnceLock<Option<FaultPlan>> = OnceLock::new();
    PLAN.get_or_init(|| {
        let spec = std::env::var(FAULT_ENV).ok()?;
        let plan = FaultPlan::parse(&spec);
        if plan.is_none() {
            tracing::warn!(
                code = "SESSION_CLEANUP_DEBUG_FAULT_INVALID",
                env = FAULT_ENV,
                "ignoring invalid debug cleanup fault spec; expected <pid>[:<count>]"
            );
        }
        plan
    })
    .as_ref()
}

/// Returns the pid the teardown of `process_ids` must report as a survivor,
/// when the debug fault is armed for it.
pub(super) fn forced_failure_pid(process_ids: &[u32]) -> Option<u32> {
    let pid = global_plan()?.consume_for(process_ids)?;
    tracing::warn!(
        code = "SESSION_CLEANUP_DEBUG_FAULT_INJECTED",
        pid,
        env = FAULT_ENV,
        "debug build: forcing session process teardown to report failure for this pid"
    );
    Some(pid)
}
