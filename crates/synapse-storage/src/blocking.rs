//! Runs blocking storage maintenance off the async workers (#60).
//!
//! GC and disk-pressure passes scan every key of every column family and run
//! `compact_range_cf`, which can take minutes under storage stress. They used
//! to run inline on a tokio worker every few minutes, stalling every other task
//! scheduled on it. They now run on tokio's blocking pool.

/// Runs `work` on the blocking pool and returns its result. A panicked or
/// cancelled job is logged and reported as `None`; the maintenance loop
/// carries on with its next tick.
pub async fn run_blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::error!(
                code = "STORAGE_MAINTENANCE_JOB_FAILED",
                job = what,
                error = %error,
                "blocking storage maintenance job did not complete"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "blocking_tests.rs"]
mod blocking_tests;
