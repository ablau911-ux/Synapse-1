//! Browser-seat startup deadline and failed-startup teardown with physical
//! readback (#73).
//!
//! One `StartupDeadline` bounds every startup wait (DevTools probe, WebSocket
//! handshake, initial CDP replies). A startup that fails or misses it is torn
//! down inside a separate allowance: kill the child, await its exit, remove the
//! private profile (retrying while files are briefly locked), then read both
//! back. Only a confirmed readback may be reported as "torn down".

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use tokio::{process::Child, time::Instant};

use crate::{
    BrowserRuntimeConfig, SeatError, SeatId, TeardownStatus, Viewport,
    profile_guard::cleanup_profile,
};

/// Starts the owned browser child. Production spawns Chromium; tests inject a
/// fake child through `BrowserBackendFactory::with_spawner`.
pub(crate) type ChildSpawner = fn(&BrowserRuntimeConfig, &Path, Viewport) -> std::io::Result<Child>;

/// What the factory hands to each launch.
#[derive(Clone, Copy)]
pub(crate) struct LaunchPolicy {
    pub(crate) spawner: ChildSpawner,
    /// Allowance for kill, exit wait, profile removal and readback after a
    /// failed startup. The supervisor passes
    /// `SeatSupervisorConfig::startup_teardown_allowance`.
    pub(crate) teardown: Duration,
}

/// Child pids of launches still in progress, by seat (#73). Every entry is
/// owned by a `LedgerEntry` guard held by its launch, so it disappears on every
/// exit path, including a dropped launch future. The supervisor backstop takes
/// its `startup_resources` snapshot while the creation future is still alive,
/// before it drops it; that snapshot is the hand-over to the reporting path.
#[derive(Default)]
pub(crate) struct SpawnLedger(Mutex<HashMap<SeatId, u32>>);

/// Removes its ledger entry when dropped.
pub(crate) struct LedgerEntry<'a> {
    ledger: &'a SpawnLedger,
    seat_id: SeatId,
}

impl Drop for LedgerEntry<'_> {
    fn drop(&mut self) {
        self.ledger.entries().remove(&self.seat_id);
    }
}

impl SpawnLedger {
    fn entries(&self) -> MutexGuard<'_, HashMap<SeatId, u32>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records the launch's child; the entry lives exactly as long as the guard.
    #[must_use = "dropping the guard removes the entry at once"]
    pub(crate) fn record(&self, seat_id: SeatId, pid: Option<u32>) -> LedgerEntry<'_> {
        if let Some(pid) = pid {
            self.entries().insert(seat_id, pid);
        }
        LedgerEntry {
            ledger: self,
            seat_id,
        }
    }

    pub(crate) fn pid(&self, seat_id: SeatId) -> Option<u32> {
        self.entries().get(&seat_id).copied()
    }
}

pub(crate) fn pid_text(pid: Option<u32>) -> String {
    pid.map_or_else(|| "unknown".to_owned(), |pid| pid.to_string())
}

/// Physical resources of a starting seat, named in every pending report.
pub(crate) fn resources_text(profile_root: &Path, pid: Option<u32>) -> String {
    format!(
        "child pid {}; private profile {}",
        pid_text(pid),
        profile_root.display()
    )
}

const REMOVAL_RETRY: Duration = Duration::from_millis(100);
const FAR_FUTURE: Duration = Duration::from_secs(60 * 60 * 24 * 365);

fn instant_after(duration: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration)
        .unwrap_or_else(|| now + FAR_FUTURE)
}

pub(crate) fn profile_root_for(config: &BrowserRuntimeConfig, seat_id: SeatId) -> PathBuf {
    config.runtime_root.join(format!("browser-seat-{seat_id}"))
}

/// One deadline shared by every startup wait of a browser seat, plus the
/// teardown allowance used when startup fails.
#[derive(Clone, Copy)]
pub(crate) struct StartupDeadline {
    at: Instant,
    timeout_ms: u64,
    pub(crate) teardown: Duration,
}

impl StartupDeadline {
    pub(crate) fn start(timeout_ms: u64, teardown: Duration) -> Self {
        Self {
            at: instant_after(Duration::from_millis(timeout_ms)),
            timeout_ms,
            teardown,
        }
    }

    /// The typed expiry error. Its teardown is filled in by `with_teardown`
    /// once the partial backend has been torn down and read back.
    pub(crate) fn expired(self) -> SeatError {
        SeatError::StartupTimeout {
            timeout_ms: self.timeout_ms,
            teardown: TeardownStatus::Pending("teardown not attempted yet".to_owned()),
        }
    }

    pub(crate) fn has_passed(self) -> bool {
        Instant::now() >= self.at
    }

    /// Runs `future` until the deadline; on expiry the future is dropped.
    pub(crate) async fn bound<T>(
        self,
        future: impl Future<Output = Result<T, SeatError>>,
    ) -> Result<T, SeatError> {
        tokio::time::timeout_at(self.at, future)
            .await
            .map_err(|_| self.expired())?
    }
}

/// Attach the physical teardown readback to a failed startup's error. A
/// non-timeout error keeps its type unless teardown is still pending.
pub(crate) fn with_teardown(error: SeatError, teardown: TeardownStatus) -> SeatError {
    match error {
        SeatError::StartupTimeout { timeout_ms, .. } => SeatError::StartupTimeout {
            timeout_ms,
            teardown,
        },
        error => match teardown {
            TeardownStatus::Confirmed => error,
            TeardownStatus::Pending(detail) => {
                SeatError::backend(format!("{error}; startup teardown pending: {detail}"))
            }
        },
    }
}

/// Kill the child, await its exit, remove the private profile with retries,
/// then read both back, all within `allowance`.
pub(crate) async fn teardown_owned(
    process: &mut Child,
    runtime_root: &Path,
    profile_root: &Path,
    allowance: Duration,
) -> TeardownStatus {
    let pid = process.id();
    let deadline = instant_after(allowance);
    let _kill_result = process.start_kill();
    let waited = tokio::time::timeout_at(deadline, process.wait()).await;
    let removal = remove_profile_until(runtime_root, profile_root, deadline).await;
    let exited = matches!(waited, Ok(Ok(_))) || matches!(process.try_wait(), Ok(Some(_)));
    let present = tokio::fs::try_exists(profile_root).await.unwrap_or(true);
    if exited && !present {
        return TeardownStatus::Confirmed;
    }
    TeardownStatus::Pending(format!(
        "child pid {} exited={exited}; private profile {} present={present}{}",
        pid_text(pid),
        profile_root.display(),
        removal.err().map_or_else(String::new, |error| format!(
            "; last removal error: {error}"
        ))
    ))
}

/// Retry profile removal until it succeeds or `deadline` passes.
pub(crate) async fn remove_profile_until(
    runtime_root: &Path,
    profile_root: &Path,
    deadline: Instant,
) -> Result<(), SeatError> {
    loop {
        let attempt =
            tokio::time::timeout_at(deadline, cleanup_profile(runtime_root, profile_root))
                .await
                .unwrap_or_else(|_| {
                    Err(SeatError::backend("profile removal exceeded its allowance"))
                });
        if attempt.is_ok() || Instant::now() + REMOVAL_RETRY >= deadline {
            return attempt;
        }
        tokio::time::sleep(REMOVAL_RETRY).await;
    }
}

/// Supervisor-backstop readback for a launch future that was dropped mid-way.
/// The dropped `Child` was killed on drop but its exit cannot be awaited here,
/// so this never reports `Confirmed`. `resources` is the supervisor's
/// pre-drop snapshot (child pid + profile path); the presence readback of that
/// profile path is appended.
pub(crate) async fn abandoned_teardown(
    config: &BrowserRuntimeConfig,
    seat_id: SeatId,
    resources: &str,
    allowance: Duration,
) -> TeardownStatus {
    let profile_root = profile_root_for(config, seat_id);
    let deadline = instant_after(allowance);
    let removal = remove_profile_until(&config.runtime_root, &profile_root, deadline).await;
    let present = tokio::fs::try_exists(&profile_root).await.unwrap_or(true);
    TeardownStatus::Pending(format!(
        "startup abandoned at the supervisor cutoff; child exit unconfirmed (kill-on-drop issued); {resources}; profile present={present}{}",
        removal.err().map_or_else(String::new, |error| format!(
            "; last removal error: {error}"
        ))
    ))
}
