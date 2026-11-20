//! Ownership of a private browser-seat profile directory.
//!
//! A seat's profile directory is created before Chromium is spawned and is only
//! handed to a live `BrowserBackend` after startup succeeds. Between those two
//! points the launch future can fail or be cancelled (supervisor deadline,
//! session teardown), so the directory is guarded: dropping an armed guard
//! schedules removal instead of leaking an unregistered directory.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::SeatError;

/// Removal retries: Chromium (or a virus scanner) can briefly hold files open
/// after the owning process was killed.
const DETACHED_CLEANUP_ATTEMPTS: u32 = 25;
const DETACHED_CLEANUP_RETRY_MS: u64 = 200;

pub(crate) async fn cleanup_profile(
    runtime_root: &Path,
    profile_root: &Path,
) -> Result<(), SeatError> {
    ensure_owned_path(runtime_root, profile_root)?;
    match tokio::fs::remove_dir_all(profile_root).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(remove_error(profile_root, &error)),
    }
}

fn cleanup_profile_blocking(runtime_root: &Path, profile_root: &Path) -> Result<(), SeatError> {
    ensure_owned_path(runtime_root, profile_root)?;
    match std::fs::remove_dir_all(profile_root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(remove_error(profile_root, &error)),
    }
}

fn ensure_owned_path(runtime_root: &Path, profile_root: &Path) -> Result<(), SeatError> {
    if profile_root.starts_with(runtime_root) && profile_root != runtime_root {
        Ok(())
    } else {
        Err(SeatError::backend(format!(
            "refusing to remove browser-seat profile outside its runtime root: {}",
            profile_root.display()
        )))
    }
}

fn remove_error(profile_root: &Path, error: &std::io::Error) -> SeatError {
    SeatError::backend(format!(
        "remove browser-seat profile {}: {error}",
        profile_root.display()
    ))
}

/// Remove the profile without awaiting: used from `Drop`, where the owner can
/// neither await nor fail. Retries on the ambient runtime; with no runtime it
/// removes once, synchronously. Failures are logged, never silent.
pub(crate) fn detach_profile_cleanup(runtime_root: PathBuf, profile_root: PathBuf) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        log_leak(
            &profile_root,
            cleanup_profile_blocking(&runtime_root, &profile_root),
        );
        return;
    };
    runtime.spawn(async move {
        let mut last = Ok(());
        for attempt in 0..DETACHED_CLEANUP_ATTEMPTS {
            last = cleanup_profile(&runtime_root, &profile_root).await;
            if last.is_ok() {
                return;
            }
            if attempt + 1 < DETACHED_CLEANUP_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(DETACHED_CLEANUP_RETRY_MS)).await;
            }
        }
        log_leak(&profile_root, last);
    });
}

fn log_leak(profile_root: &Path, result: Result<(), SeatError>) {
    if let Err(error) = result {
        tracing::warn!(
            profile = %profile_root.display(),
            %error,
            "browser-seat profile directory could not be removed"
        );
    }
}

/// Armed from before the profile directory exists until the live backend takes
/// ownership. Dropping it while armed schedules removal of the directory.
pub(crate) struct ProfileGuard {
    runtime_root: PathBuf,
    profile_root: PathBuf,
    armed: bool,
}

impl ProfileGuard {
    pub(crate) const fn new(runtime_root: PathBuf, profile_root: PathBuf) -> Self {
        Self {
            runtime_root,
            profile_root,
            armed: true,
        }
    }

    /// Remove now. Disarms only on success; a failed removal stays armed so
    /// `Drop` retries it in the background.
    pub(crate) async fn cleanup(&mut self) -> Result<(), SeatError> {
        cleanup_profile(&self.runtime_root, &self.profile_root).await?;
        self.armed = false;
        Ok(())
    }

    /// The live backend now owns (and will tear down) the directory.
    pub(crate) const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        if self.armed {
            detach_profile_cleanup(
                std::mem::take(&mut self.runtime_root),
                std::mem::take(&mut self.profile_root),
            );
        }
    }
}
