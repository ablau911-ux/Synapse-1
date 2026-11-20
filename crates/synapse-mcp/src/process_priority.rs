//! #146: under full CPU load (one cargo build on all cores) the HTTP daemon ran
//! at normal priority and was starved like any other busy process: MCP
//! initialize and tools/list took 10-19 s and Codex's 30 s handshake timed out.
//! The daemon is mostly idle and serves interactive agents, so the tokio
//! runtime worker threads that serve MCP run one level above normal. The
//! process class stays NORMAL: raising it would also lift RocksDB compaction,
//! OCR/capture readback and the blocking pool above the user's foreground apps.
//! `SYNAPSE_PROCESS_PRIORITY` overrides: `normal` (no boost at all),
//! `above_normal` / `high` (whole process class, the old behaviour), unset or
//! `worker_above_normal` (default: worker threads only).
use std::{
    cell::Cell,
    sync::atomic::{AtomicBool, Ordering},
};

const PRIORITY_ENV: &str = "SYNAPSE_PROCESS_PRIORITY";
const DEFAULT_PRIORITY: Priority = Priority::WorkerAboveNormal;

/// Set once the configured policy wants runtime worker threads boosted.
static WORKER_BOOST: AtomicBool = AtomicBool::new(false);
static WORKER_BOOST_FAILURE_LOGGED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static THREAD_BOOSTED: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Priority {
    /// Nothing is raised.
    Normal,
    /// Process class NORMAL; tokio runtime worker threads one level up.
    WorkerAboveNormal,
    /// Whole-process class (affects every thread of the daemon).
    AboveNormal,
    High,
}

impl Priority {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::WorkerAboveNormal => "worker_above_normal",
            Self::AboveNormal => "above_normal",
            Self::High => "high",
        }
    }

    /// Whether runtime worker threads get THREAD_PRIORITY_ABOVE_NORMAL.
    pub(crate) const fn boosts_workers(self) -> bool {
        matches!(self, Self::WorkerAboveNormal)
    }
}

/// Resolve the configured class. Unknown values fall back to the default and
/// are reported, never silently honoured.
pub(crate) fn resolve(raw: Option<&str>) -> (Priority, Option<String>) {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return (DEFAULT_PRIORITY, None);
    };
    match raw.to_ascii_lowercase().as_str() {
        "normal" => (Priority::Normal, None),
        "worker_above_normal" | "workerabovenormal" => (Priority::WorkerAboveNormal, None),
        "above_normal" | "abovenormal" => (Priority::AboveNormal, None),
        "high" => (Priority::High, None),
        _ => (DEFAULT_PRIORITY, Some(raw.to_owned())),
    }
}

/// Build the daemon's tokio runtime with the worker-thread priority hook.
/// `on_thread_unpark` fires only on scheduler workers, never on the
/// `spawn_blocking` pool (RocksDB, OCR, readback), which stays NORMAL.
pub(crate) fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .on_thread_unpark(raise_current_thread_if_enabled)
        .build()
}

/// Raise the calling thread once, if the policy asks for worker boosting.
pub(crate) fn raise_current_thread_if_enabled() {
    if !WORKER_BOOST.load(Ordering::Acquire) || THREAD_BOOSTED.get() {
        return;
    }
    // Success or failure, do not retry on every unpark.
    THREAD_BOOSTED.set(true);
    if let Err(error) = set_current_thread_above_normal()
        && !WORKER_BOOST_FAILURE_LOGGED.swap(true, Ordering::AcqRel)
    {
        tracing::warn!(
            code = "MCP_WORKER_PRIORITY_SET_FAILED", %error,
            "could not raise a runtime worker thread priority; continuing at normal"
        );
    }
}

/// Apply the configured priority to this process and log the readback.
pub(crate) fn apply_from_env() {
    let raw = std::env::var(PRIORITY_ENV).ok();
    let (priority, rejected) = resolve(raw.as_deref());
    if let Some(value) = rejected {
        tracing::warn!(
            code = "MCP_PROCESS_PRIORITY_INVALID",
            value = %value,
            fallback = priority.as_str(),
            "unknown {PRIORITY_ENV}; using default"
        );
    }
    WORKER_BOOST.store(priority.boosts_workers(), Ordering::Release);
    // The thread running block_on is not a scheduler worker; raise it directly.
    raise_current_thread_if_enabled();
    match set_current_process_priority(priority) {
        Ok(()) => tracing::info!(
            code = "MCP_PROCESS_PRIORITY_SET",
            priority = priority.as_str(),
            pid = std::process::id(),
            "daemon process priority policy applied"
        ),
        Err(error) => tracing::warn!(
            code = "MCP_PROCESS_PRIORITY_SET_FAILED",
            priority = priority.as_str(),
            %error,
            "could not change daemon process priority; continuing at inherited class"
        ),
    }
}

#[cfg(windows)]
fn set_current_process_priority(priority: Priority) -> anyhow::Result<()> {
    use windows::Win32::System::Threading::{
        ABOVE_NORMAL_PRIORITY_CLASS, GetCurrentProcess, HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        SetPriorityClass,
    };
    let class = match priority {
        Priority::Normal | Priority::WorkerAboveNormal => NORMAL_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
    };
    // SAFETY: GetCurrentProcess returns a pseudo-handle valid for this process
    // with full access; SetPriorityClass only changes this process's class.
    unsafe { SetPriorityClass(GetCurrentProcess(), class) }?;
    Ok(())
}

#[cfg(windows)]
fn set_current_thread_above_normal() -> anyhow::Result<()> {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    // SAFETY: GetCurrentThread returns a pseudo-handle valid for the calling
    // thread; SetThreadPriority only changes that thread's priority.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) }?;
    Ok(())
}

#[cfg(not(windows))]
fn set_current_process_priority(_priority: Priority) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
fn set_current_thread_above_normal() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "process_priority_tests.rs"]
mod tests;
#[cfg(all(test, windows))]
#[path = "tests/process_priority_workers_146.rs"]
mod worker_tests;
