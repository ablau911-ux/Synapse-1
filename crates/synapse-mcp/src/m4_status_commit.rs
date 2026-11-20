//! Ordering and staging helpers for the durable job `status.json` (#41).
//!
//! Monitor, cancel, status-readback and reconcile paths all rewrite one job's status
//! file. Writers of the same file are serialized with a per-path lock, each write stages
//! under its own unique name, and a stale live status (`running`, `cancel_requested`,
//! `finalizing`) never replaces a committed terminal one.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use super::{ActRunShellJobStatus, read_shell_status_bytes, shell_job_terminal_status};

static WRITE_LOCKS: Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>> = Mutex::new(BTreeMap::new());
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    static STAGING_PATHS: std::cell::RefCell<Option<std::collections::VecDeque<PathBuf>>> =
        const { std::cell::RefCell::new(None) };
}

/// Override this thread's real allocator, restoring any outer override on unwind.
#[cfg(test)]
pub(super) fn with_staging_paths_for_test<T>(
    paths: &[std::path::PathBuf],
    action: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<std::collections::VecDeque<PathBuf>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            STAGING_PATHS.with(|paths| {
                paths.replace(self.0.take());
            });
        }
    }
    let _restore =
        Restore(STAGING_PATHS.with(|slot| slot.replace(Some(paths.iter().cloned().collect()))));
    action()
}

/// Run `write` while holding the lock of the status file at `path`.
pub(super) fn with_lock<T>(path: &Path, write: impl FnOnce() -> T) -> T {
    let lock = {
        let mut locks = WRITE_LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
        // Drop locks no writer holds any more so the registry does not grow per job.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(locks.entry(path.to_path_buf()).or_default())
    };
    let _held = lock.lock().unwrap_or_else(PoisonError::into_inner);
    write()
}

/// A staging path unique to this write, beside `path`: `<name>.<pid>-<seq>.tmp`.
pub(super) fn staging_path(path: &Path) -> PathBuf {
    #[cfg(test)]
    if let Some(staging) = STAGING_PATHS.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|paths| match paths.pop_front() {
                Some(path) => path,
                None => panic!("unexpected staging allocation: injected paths exhausted"),
            })
    }) {
        return staging;
    }
    let name = path
        .file_name()
        .map_or_else(|| "status".into(), |name| name.to_string_lossy());
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!("{name}.{}-{sequence}.tmp", std::process::id()))
}

/// Reserve a unique staging file, tolerating files left by an earlier PID generation.
pub(super) fn create_staging_file(path: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    for _ in 0..8 {
        let staging = staging_path(path);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
        {
            Ok(file) => return Ok((staging, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique status staging file",
    ))
}

/// True while some writer's staging file for `path` exists, i.e. a replace is in flight.
pub(super) fn staging_in_flight(path: &Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    std::fs::read_dir(parent).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            entry_name.starts_with(&prefix) && entry_name.ends_with(".tmp")
        })
    })
}

/// The currently committed status, or `None` when absent or unreadable.
fn committed_status(path: &Path) -> Option<ActRunShellJobStatus> {
    let bytes = read_shell_status_bytes(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// True when `candidate` is a stale live status that must not replace the committed
/// terminal status at `path`.
pub(super) fn stale_live_over_terminal(path: &Path, candidate: &ActRunShellJobStatus) -> bool {
    if shell_job_terminal_status(&candidate.status) {
        return false;
    }
    committed_status(path).is_some_and(|latest| shell_job_terminal_status(&latest.status))
}
