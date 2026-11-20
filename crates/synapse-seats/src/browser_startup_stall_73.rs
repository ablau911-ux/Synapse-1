//! #73: drive `SeatSupervisor::create` through the real browser path
//! (`BrowserBackendFactory::create` -> `launch_with` -> `wait_for_cdp` ->
//! `finish_startup`) with the fake child and scripted DevTools endpoint from
//! `fake_devtools_73`. Each phase can be delayed or stalled, so one test proves
//! a single total startup deadline. Every test reads the OS process table (fail
//! closed) and the runtime directory back separately.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant as StdInstant},
};

use super::fake_devtools_73::*;
use super::*;
use crate::{OwnerId, SeatState, SeatSupervisor, SeatView};

/// Allowed lateness past an expected deadline (spawn, kill + exit wait, removal).
const MARGIN: Duration = Duration::from_millis(750);
/// Timer and scheduling tolerance on the C5 budget itself (measurement only).
const TIMER_TOLERANCE: Duration = Duration::from_millis(100);
const NAVIGATE_URL: &str = "https://example.invalid/seat-73";

struct Run {
    result: Result<SeatView, SeatError>,
    elapsed: Duration,
    pid: u32,
    /// The one starting seat's private profile root, read at the stalled phase.
    profile_root: PathBuf,
    /// A file in that profile held open without delete sharing.
    locked: Option<std::fs::File>,
}

/// Starts `seat_create`, waits until startup reached `phase`, proves the exact
/// fake child is alive in the OS process table, optionally locks a profile
/// file, then awaits the outcome.
async fn create_until(
    fake: &Fake,
    supervisor: &SeatSupervisor,
    initial_url: Option<&str>,
    phase: &'static str,
    lock_profile: bool,
) -> TestResult<Run> {
    let create = tokio::spawn({
        let (supervisor, config) = (supervisor.clone(), browser_seat(initial_url));
        async move {
            let started = StdInstant::now();
            let result = supervisor.create(OwnerId::new("startup-73"), config).await;
            (result, started.elapsed())
        }
    });
    fake.wait_for(phase).await?;
    let pid = fake.pid()?;
    assert!(
        cleanup_tests::process_listed(pid).await?,
        "precondition: fake child {pid} alive at {phase}"
    );
    let profile_root = only_profile_root(&fake.runtime_root).await?;
    let locked = if lock_profile {
        Some(lock_profile_file(&profile_root)?)
    } else {
        None
    };
    let joined = tokio::time::timeout(TEST_BOUND, create).await;
    let (result, elapsed) =
        joined.map_err(|_| "seat_create still pending: startup is unbounded")??;
    Ok(Run {
        result,
        elapsed,
        pid,
        profile_root,
        locked,
    })
}

/// `elapsed` must not precede the startup deadline, must stay under `upper`,
/// and must never exceed the C5 budget (startup + request) plus timer tolerance.
fn assert_between(elapsed: Duration, upper: Duration) {
    let startup = Duration::from_millis(STARTUP_MS);
    let c5 = Duration::from_millis(STARTUP_MS + REQUEST_MS) + TIMER_TOLERANCE;
    assert!(
        elapsed >= startup,
        "ended before the startup deadline: {elapsed:?}"
    );
    assert!(elapsed <= upper, "ended late: {elapsed:?} > {upper:?}");
    assert!(
        elapsed <= c5,
        "exceeded the C5 budget: {elapsed:?} > {c5:?}"
    );
}

/// The browser's own teardown allowance, derived as the supervisor derives it.
fn teardown_allowance(fake: &Fake) -> TestResult<Duration> {
    Ok(supervisor_config(fake)?.startup_teardown_allowance())
}

/// The typed timeout, then the next admission on the same supervisor succeeds.
async fn assert_unregistered_then_admits(fake: &Fake, supervisor: &SeatSupervisor) -> TestResult {
    assert!(
        supervisor.list_all_for_supervision().await.is_empty(),
        "no seat may be registered"
    );
    *fake.script.lock().await = FAST;
    let owner = OwnerId::new("startup-73-next");
    let next = tokio::time::timeout(
        TEST_BOUND,
        supervisor.create(owner.clone(), browser_seat(None)),
    )
    .await
    .map_err(|_| "next admission hung")??;
    assert_eq!(
        next.state,
        SeatState::Ready,
        "the capacity slot must be free again"
    );
    let closed = supervisor.close(&owner, next.id, next.generation).await?;
    assert_eq!(closed.state, SeatState::Closed);
    Ok(())
}

/// Full #73 acceptance for a stall at `phase`: typed timeout at the one deadline,
/// confirmed teardown, child gone, profile gone, nothing registered, next admission OK.
async fn assert_stall_tears_down(
    script: Script,
    url: Option<&str>,
    phase: &'static str,
) -> TestResult {
    let fake = fake_devtools(script).await?;
    let supervisor = supervisor(&fake)?;
    let run = create_until(&fake, &supervisor, url, phase, false).await?;
    match &run.result {
        Err(SeatError::StartupTimeout {
            timeout_ms,
            teardown,
        }) => {
            assert_eq!(*timeout_ms, STARTUP_MS);
            assert_eq!(*teardown, TeardownStatus::Confirmed);
        }
        other => panic!("stall at {phase}: expected confirmed StartupTimeout, got {other:?}"),
    }
    assert_between(run.elapsed, Duration::from_millis(STARTUP_MS) + MARGIN);
    let leftovers = cleanup_tests::dir_entries(&fake.runtime_root).await?;
    assert!(
        leftovers.is_empty(),
        "confirmed teardown left profiles: {leftovers:?}"
    );
    assert!(
        cleanup_tests::wait_process_gone(run.pid).await?,
        "fake child {} survived",
        run.pid
    );
    assert_unregistered_then_admits(&fake, &supervisor).await?;
    fake.finish().await
}

#[tokio::test]
async fn must_fail_phases_share_one_total_startup_deadline_73() -> TestResult {
    // Each phase alone fits easily; together (4 x 400 ms) they exceed 1500 ms. A
    // deadline restarted per phase, or a launch that skips finish_startup, succeeds.
    let script = Script {
        probe: Reply::AfterMs(400),
        handshake: Handshake::AfterMs(400),
        metrics: Reply::AfterMs(400),
        navigate: Reply::AfterMs(400),
    };
    assert_stall_tears_down(script, Some(NAVIGATE_URL), "navigate").await
}

#[tokio::test]
async fn must_fail_devtools_probe_never_ready_73() -> TestResult {
    let script = Script {
        probe: Reply::Never,
        ..FAST
    };
    assert_stall_tears_down(script, None, "probe").await
}

#[tokio::test]
async fn must_fail_silent_websocket_handshake_73() -> TestResult {
    let script = Script {
        handshake: Handshake::Silent,
        ..FAST
    };
    assert_stall_tears_down(script, None, "handshake").await
}

#[tokio::test]
async fn must_fail_partial_websocket_handshake_73() -> TestResult {
    let script = Script {
        handshake: Handshake::Partial,
        ..FAST
    };
    assert_stall_tears_down(script, None, "handshake").await
}

#[tokio::test]
async fn must_fail_stalled_first_cdp_reply_73() -> TestResult {
    let script = Script {
        metrics: Reply::Never,
        ..FAST
    };
    assert_stall_tears_down(script, None, "metrics").await
}

#[tokio::test]
async fn must_fail_stalled_initial_navigate_reply_73() -> TestResult {
    let script = Script {
        navigate: Reply::Never,
        ..FAST
    };
    assert_stall_tears_down(script, Some(NAVIGATE_URL), "navigate").await
}

async fn only_profile_root(runtime_root: &Path) -> TestResult<PathBuf> {
    let seats = cleanup_tests::dir_entries(runtime_root).await?;
    let [seat] = seats.as_slice() else {
        return Err(format!("expected one starting seat profile, found {seats:?}").into());
    };
    Ok(runtime_root.join(seat))
}

/// Holds a file in the private profile open without delete sharing (os error 32).
fn lock_profile_file(profile_root: &Path) -> TestResult<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .open(profile_root.join("profile").join("locked-73"))?;
    Ok(lock)
}

#[tokio::test]
async fn must_fail_locked_profile_reports_pending_teardown_with_its_path_73() -> TestResult {
    let fake = fake_devtools(Script {
        metrics: Reply::Never,
        ..FAST
    })
    .await?;
    let supervisor = supervisor(&fake)?;
    let run = create_until(&fake, &supervisor, None, "metrics", true).await?;
    let lock = run.locked.ok_or("profile lock was not taken")?;
    let profile_root = run.profile_root;
    let (result, elapsed, pid) = (run.result, run.elapsed, run.pid);
    let path = profile_root.display().to_string();
    match &result {
        Err(SeatError::StartupTimeout {
            teardown: TeardownStatus::Pending(detail),
            ..
        }) => {
            assert!(
                detail.contains(&path),
                "pending teardown must name {path}: {detail}"
            );
            assert!(detail.contains("present=true"), "{detail}");
        }
        other => panic!("a locked profile must be reported as pending teardown, got {other:?}"),
    }
    // Pending cleanup need not exhaust its allowance; the allowance is only an
    // upper bound.
    let upper = Duration::from_millis(STARTUP_MS) + teardown_allowance(&fake)? + MARGIN;
    assert_between(elapsed, upper);
    assert!(
        tokio::fs::try_exists(&profile_root).await?,
        "the report must match the disk"
    );
    assert!(
        cleanup_tests::wait_process_gone(pid).await?,
        "the child must die even when removal fails"
    );
    assert_unregistered_then_admits(&fake, &supervisor).await?;
    drop(lock);
    let leftovers = cleanup_tests::wait_until_dir_empty(&fake.runtime_root).await?;
    assert!(
        leftovers.is_empty(),
        "drop guard must finish the removal: {leftovers:?}"
    );
    fake.finish().await
}

#[tokio::test]
async fn must_pass_responsive_startup_creates_ready_seat_and_keeps_child_73() -> TestResult {
    let fake = fake_devtools(FAST).await?;
    let supervisor = supervisor(&fake)?;
    let owner = OwnerId::new("startup-73");
    let created = tokio::time::timeout(
        TEST_BOUND,
        supervisor.create(owner.clone(), browser_seat(Some(NAVIGATE_URL))),
    )
    .await
    .map_err(|_| "responsive startup hung")??;
    assert_eq!(created.state, SeatState::Ready);
    fake.wait_for("navigate").await?;
    let pid = fake.pid()?;
    assert!(
        cleanup_tests::process_listed(pid).await?,
        "a successful startup must keep its child"
    );
    supervisor
        .close(&owner, created.id, created.generation)
        .await?;
    assert!(cleanup_tests::wait_process_gone(pid).await?);
    assert!(
        cleanup_tests::dir_entries(&fake.runtime_root)
            .await?
            .is_empty()
    );
    fake.finish().await
}

/// The real browser factory, started late so the supervisor cutoff lands while
/// its launch is mid-way, and whose abandoned-startup readback never answers:
/// forces the backstop fallback, which must still name profile path and pid.
struct LateBrowserFactory {
    inner: BrowserBackendFactory,
    delay: Duration,
}

#[async_trait::async_trait]
impl SeatBackendFactory for LateBrowserFactory {
    fn kind(&self) -> BackendKind {
        BackendKind::Browser
    }
    async fn create(
        &self,
        seat_id: SeatId,
        config: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError> {
        tokio::time::sleep(self.delay).await;
        self.inner.create(seat_id, config).await
    }
    fn startup_resources(&self, seat_id: SeatId) -> String {
        self.inner.startup_resources(seat_id)
    }
    async fn abandoned_startup_teardown(
        &self,
        _seat_id: SeatId,
        _resources: &str,
        _allowance: Duration,
    ) -> TeardownStatus {
        std::future::pending().await
    }
}

fn assert_pending_names(result: &Result<SeatView, SeatError>, path: &str, pid: u32) {
    let pid_text = format!("child pid {pid}");
    let Err(error) = result else {
        panic!("a backstop timeout must not create a seat: {result:?}");
    };
    // The MCP message is `error.to_string()`.
    let message = error.to_string();
    assert!(
        message.contains(path),
        "MCP message must name {path}: {message}"
    );
    match error {
        SeatError::StartupTimeout {
            timeout_ms,
            teardown: TeardownStatus::Pending(detail),
        } => {
            assert_eq!(*timeout_ms, STARTUP_MS + REQUEST_MS, "C5 budget");
            assert!(detail.contains(path), "must name {path}: {detail}");
            assert!(detail.contains(&pid_text), "must name {pid_text}: {detail}");
        }
        other => panic!("expected pending StartupTimeout, got {other:?}"),
    }
}

#[tokio::test]
async fn must_fail_backstop_readback_timeout_still_names_profile_path_and_pid_73() -> TestResult {
    // Cutoff at C5 - reserve = 2250 ms lands while the launch (started at
    // 1000 ms, own deadline 2500 ms) waits on a silent first reply.
    let fake = fake_devtools(Script {
        metrics: Reply::Never,
        ..FAST
    })
    .await?;
    let config = supervisor_config(&fake)?;
    let late = LateBrowserFactory {
        inner: browser_factory(&config),
        delay: Duration::from_millis(1_000),
    };
    let supervisor = SeatSupervisor::assemble(config, Arc::new(late), Vec::new())?;
    let run = create_until(&fake, &supervisor, None, "metrics", false).await?;
    let path = run.profile_root.display().to_string();
    assert_pending_names(&run.result, &path, run.pid);
    let c5 = Duration::from_millis(STARTUP_MS + REQUEST_MS);
    assert!(
        run.elapsed >= c5,
        "the readback may use the whole budget: {:?}",
        run.elapsed
    );
    assert!(
        run.elapsed <= c5 + TIMER_TOLERANCE,
        "exceeded C5: {:?}",
        run.elapsed
    );
    assert!(
        cleanup_tests::wait_process_gone(run.pid).await?,
        "kill-on-drop must end the child"
    );
    let leftovers = cleanup_tests::wait_until_dir_empty(&fake.runtime_root).await?;
    assert!(
        leftovers.is_empty(),
        "drop guards must remove the profile: {leftovers:?}"
    );
    assert_unregistered_then_admits(&fake, &supervisor).await?;
    fake.finish().await
}

fn seat_id_of(profile_root: &Path) -> TestResult<SeatId> {
    let name = profile_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("profile root has no name")?;
    let uuid = name
        .strip_prefix("browser-seat-")
        .ok_or("unexpected profile root name")?;
    Ok(SeatId(uuid::Uuid::parse_str(uuid)?))
}

#[tokio::test]
async fn must_fail_cancelled_create_releases_ledger_child_profile_and_capacity_73() -> TestResult {
    let fake = fake_devtools(Script {
        metrics: Reply::Never,
        ..FAST
    })
    .await?;
    let config = supervisor_config(&fake)?;
    let factory = Arc::new(browser_factory(&config));
    let shared = Arc::clone(&factory) as Arc<dyn SeatBackendFactory>;
    let supervisor = SeatSupervisor::assemble(config, shared, Vec::new())?;
    let create = tokio::spawn({
        let supervisor = supervisor.clone();
        async move {
            let owner = OwnerId::new("startup-73");
            supervisor.create(owner, browser_seat(None)).await
        }
    });
    fake.wait_for("metrics").await?;
    let pid = fake.pid()?;
    assert!(
        cleanup_tests::process_listed(pid).await?,
        "precondition: child {pid} alive"
    );
    let seat = seat_id_of(&only_profile_root(&fake.runtime_root).await?)?;
    assert_eq!(
        factory.ledger.pid(seat),
        Some(pid),
        "precondition: ledger holds the child"
    );
    // Cancel well before the startup deadline and the supervisor cutoff.
    create.abort();
    assert!(create.await.is_err_and(|error| error.is_cancelled()));
    assert_eq!(
        factory.ledger.pid(seat),
        None,
        "a dropped create must not leak its entry"
    );
    assert!(
        cleanup_tests::wait_process_gone(pid).await?,
        "child {pid} survived the cancel"
    );
    let leftovers = cleanup_tests::wait_until_dir_empty(&fake.runtime_root).await?;
    assert!(
        leftovers.is_empty(),
        "cancelled create left profiles: {leftovers:?}"
    );
    assert_unregistered_then_admits(&fake, &supervisor).await?;
    fake.finish().await
}
