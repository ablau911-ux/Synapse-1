//! #73 C5: `SeatSupervisor::create` must bound the whole backend startup. A factory
//! whose creation never resolves must return a typed `SeatError::StartupTimeout`
//! within startup_timeout_ms + request_timeout_ms TOTAL, covering creation,
//! cancellation (the abandoned future is dropped), the factory's physical
//! teardown readback and the return; the report must be honest and name the
//! physical resources, and the capacity permit must be returned.
//! Drives the real supervisor create path with in-memory stalling factories.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tokio::sync::Notify;

use super::{SeatBackend, SeatBackendFactory, SeatSupervisor};
use crate::{
    ActionDetail, BackendIdentity, BackendKind, BrowserRuntimeConfig, Frame, MockSeatConfig,
    OwnerId, SeatAction, SeatConfig, SeatError, SeatGeneration, SeatId, SeatState,
    SeatSupervisorConfig, SeatView, TeardownStatus,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const STARTUP_MS: u64 = 300;
const REQUEST_MS: u64 = 300;
/// C5 contract: startup + request, total.
const C5_BUDGET_MS: u64 = STARTUP_MS + REQUEST_MS;
/// Timer and scheduling tolerance on the C5 budget (measurement only).
const TIMER_TOLERANCE: Duration = Duration::from_millis(100);
const SYNTHETIC_PID: &str = "child pid 4242";
const SYNTHETIC_PATH: &str = "C:/synthetic-73/browser-seat-";
/// Only guards against a hang; timing is asserted with `assert_c5`.
const TEST_BOUND: Duration = Duration::from_secs(5);

struct ReadyBackend;

#[async_trait]
impl SeatBackend for ReadyBackend {
    fn identity(&self) -> BackendIdentity {
        BackendIdentity {
            kind: BackendKind::Mock,
            target: "startup-73".to_owned(),
            process_id: None,
        }
    }
    async fn act(&mut self, _action: SeatAction) -> Result<ActionDetail, SeatError> {
        Ok(ActionDetail::TextInserted { byte_count: 1 })
    }
    async fn capture(&mut self) -> Result<Frame, SeatError> {
        Err(SeatError::backend("capture is not used by #73 tests"))
    }
    async fn release_inputs(&mut self) -> Result<(), SeatError> {
        Ok(())
    }
    async fn close(&mut self) -> Result<(), SeatError> {
        Ok(())
    }
}

/// Set when the abandoned startup future is dropped (stands in for child/profile cleanup).
struct DropProbe(Arc<AtomicBool>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// How the stalling factory answers the supervisor's teardown readback.
#[derive(Clone, Copy)]
enum Readback {
    /// Confirmed only once the abandoned future was really dropped.
    DropProbe,
    /// Teardown could not be confirmed; names a physical path.
    ReportsPending,
    /// The readback itself never answers.
    Hangs,
}

/// The first `failures` calls fail fast; the next `stalls` calls never resolve; the rest
/// succeed after `delay`.
struct StallingFactory {
    failures: AtomicUsize,
    stalls: AtomicUsize,
    delay: Duration,
    readback: Readback,
    entered: Notify,
    abandoned: Arc<AtomicBool>,
}

fn take_one(counter: &AtomicUsize) -> bool {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(1)
        })
        .is_ok()
}

#[async_trait]
impl SeatBackendFactory for StallingFactory {
    fn kind(&self) -> BackendKind {
        BackendKind::Mock
    }
    async fn create(
        &self,
        _seat_id: SeatId,
        _config: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError> {
        if take_one(&self.failures) {
            return Err(SeatError::backend("synthetic #73 startup failure"));
        }
        let stall = take_one(&self.stalls);
        self.entered.notify_one();
        if stall {
            let _probe = DropProbe(Arc::clone(&self.abandoned));
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(self.delay).await;
        Ok(Box::new(ReadyBackend))
    }
    async fn abandoned_startup_teardown(
        &self,
        _seat_id: SeatId,
        resources: &str,
        _allowance: Duration,
    ) -> TeardownStatus {
        match self.readback {
            Readback::DropProbe if self.abandoned.load(Ordering::SeqCst) => {
                TeardownStatus::Confirmed
            }
            Readback::DropProbe => TeardownStatus::Pending("creation future still alive".into()),
            Readback::ReportsPending => {
                TeardownStatus::Pending(format!("{resources} present=true"))
            }
            Readback::Hangs => std::future::pending().await,
        }
    }
    fn startup_resources(&self, seat_id: SeatId) -> String {
        format!("{SYNTHETIC_PID}; private profile {SYNTHETIC_PATH}{seat_id}")
    }
}

/// A factory that stalls and keeps the trait's default readback.
struct NoReadbackFactory;

#[async_trait]
impl SeatBackendFactory for NoReadbackFactory {
    fn kind(&self) -> BackendKind {
        BackendKind::Mock
    }
    async fn create(
        &self,
        _seat_id: SeatId,
        _config: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError> {
        std::future::pending().await
    }
}

fn factory(failures: usize, stalls: usize, readback: Readback) -> Arc<StallingFactory> {
    Arc::new(StallingFactory {
        failures: AtomicUsize::new(failures),
        stalls: AtomicUsize::new(stalls),
        delay: Duration::from_millis(100),
        readback,
        entered: Notify::new(),
        abandoned: Arc::new(AtomicBool::new(false)),
    })
}

async fn supervisor(factory: Arc<dyn SeatBackendFactory>) -> TestResult<SeatSupervisor> {
    let supervisor = SeatSupervisor::new(SeatSupervisorConfig {
        browser: BrowserRuntimeConfig {
            chromium_executable: PathBuf::from("unused-startup-73.exe"),
            runtime_root: PathBuf::from("unused-startup-73"),
            startup_timeout_ms: STARTUP_MS,
        },
        max_seats: 1,
        per_seat_queue_capacity: 1,
        request_timeout_ms: REQUEST_MS,
    })?;
    supervisor.register_backend(factory).await?;
    Ok(supervisor)
}

fn mock() -> SeatConfig {
    SeatConfig::Mock(MockSeatConfig {})
}

/// One timed create; panics if it is still pending at `TEST_BOUND`.
async fn timed_create(
    supervisor: &SeatSupervisor,
    owner: &OwnerId,
) -> (Result<SeatView, SeatError>, Duration) {
    let started = Instant::now();
    let outcome = tokio::time::timeout(TEST_BOUND, supervisor.create(owner.clone(), mock())).await;
    let Ok(result) = outcome else {
        panic!("seat_create still pending after {TEST_BOUND:?}: startup is unbounded");
    };
    (result, started.elapsed())
}

/// C5: never before the backend's own startup deadline, never after
/// startup + request (plus timer tolerance).
fn assert_c5(elapsed: Duration) {
    let startup = Duration::from_millis(STARTUP_MS);
    let c5 = Duration::from_millis(C5_BUDGET_MS) + TIMER_TOLERANCE;
    assert!(
        elapsed >= startup,
        "cut before the startup deadline: {elapsed:?}"
    );
    assert!(
        elapsed <= c5,
        "exceeded the C5 budget: {elapsed:?} > {c5:?}"
    );
}

fn assert_names_resources(detail: &str) {
    assert!(
        detail.contains(SYNTHETIC_PATH),
        "must name the profile path: {detail}"
    );
    assert!(
        detail.contains(SYNTHETIC_PID),
        "must name the child pid: {detail}"
    );
}

fn startup_teardown(result: Result<SeatView, SeatError>) -> TeardownStatus {
    match result {
        Err(SeatError::StartupTimeout {
            timeout_ms,
            teardown,
        }) => {
            assert_eq!(timeout_ms, C5_BUDGET_MS);
            teardown
        }
        Err(other) => panic!("expected typed StartupTimeout, got: {other}"),
        Ok(view) => panic!("a stalled startup must not create seat {}", view.id),
    }
}

async fn assert_unregistered_then_ready(supervisor: &SeatSupervisor, owner: &OwnerId) {
    assert!(
        supervisor.list_all_for_supervision().await.is_empty(),
        "no seat may be registered"
    );
    let (next, _) = timed_create(supervisor, owner).await;
    match next {
        Ok(view) => {
            assert_eq!(view.state, SeatState::Ready);
            assert_eq!(view.generation, SeatGeneration(1));
        }
        Err(error) => panic!("the capacity permit must be free again: {error}"),
    }
}

#[tokio::test]
async fn must_fail_stalled_backend_startup_times_out_with_confirmed_readback_73() -> TestResult {
    let factory = factory(0, 1, Readback::DropProbe);
    let supervisor = supervisor(Arc::clone(&factory) as Arc<dyn SeatBackendFactory>).await?;
    let owner = OwnerId::new("startup-73");
    let (result, elapsed) = timed_create(&supervisor, &owner).await;
    assert_eq!(startup_teardown(result), TeardownStatus::Confirmed);
    assert_c5(elapsed);
    assert!(
        factory.abandoned.load(Ordering::SeqCst),
        "abandoned future must be dropped"
    );
    assert_unregistered_then_ready(&supervisor, &owner).await;
    Ok(())
}

#[tokio::test]
async fn must_fail_pending_readback_is_reported_not_claimed_as_torn_down_73() -> TestResult {
    let factory = factory(0, 1, Readback::ReportsPending);
    let supervisor = supervisor(factory as Arc<dyn SeatBackendFactory>).await?;
    let owner = OwnerId::new("startup-73");
    let (result, elapsed) = timed_create(&supervisor, &owner).await;
    match startup_teardown(result) {
        TeardownStatus::Pending(detail) => assert_names_resources(&detail),
        TeardownStatus::Confirmed => panic!("a pending readback must not be reported as confirmed"),
    }
    assert_c5(elapsed);
    assert_unregistered_then_ready(&supervisor, &owner).await;
    Ok(())
}

#[tokio::test]
async fn must_fail_hanging_readback_is_bounded_and_reported_pending_73() -> TestResult {
    let factory = factory(0, 1, Readback::Hangs);
    let supervisor = supervisor(factory as Arc<dyn SeatBackendFactory>).await?;
    let owner = OwnerId::new("startup-73");
    let (result, elapsed) = timed_create(&supervisor, &owner).await;
    match startup_teardown(result) {
        TeardownStatus::Pending(detail) => {
            assert!(detail.contains("readback"), "{detail}");
            assert_names_resources(&detail);
        }
        TeardownStatus::Confirmed => panic!("a hung readback must not be reported as confirmed"),
    }
    assert_c5(elapsed);
    assert_unregistered_then_ready(&supervisor, &owner).await;
    Ok(())
}

#[tokio::test]
async fn must_fail_default_readback_never_claims_teardown_73() -> TestResult {
    let supervisor = supervisor(Arc::new(NoReadbackFactory)).await?;
    let owner = OwnerId::new("startup-73");
    let (result, elapsed) = timed_create(&supervisor, &owner).await;
    match startup_teardown(result) {
        TeardownStatus::Pending(detail) => {
            assert!(detail.contains("no physical readback"), "{detail}");
            assert!(detail.contains("reports no physical resources"), "{detail}");
        }
        TeardownStatus::Confirmed => panic!("the default readback cannot confirm teardown"),
    }
    assert_c5(elapsed);
    assert!(supervisor.list_all_for_supervision().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn must_fail_concurrent_create_during_stalled_startup_is_capacity_exhausted_73() -> TestResult
{
    let factory = factory(0, 1, Readback::DropProbe);
    let supervisor = supervisor(Arc::clone(&factory) as Arc<dyn SeatBackendFactory>).await?;
    let owner = OwnerId::new("startup-73-a");
    let first = tokio::spawn({
        let supervisor = supervisor.clone();
        let owner = owner.clone();
        async move { timed_create(&supervisor, &owner).await }
    });
    tokio::time::timeout(TEST_BOUND, factory.entered.notified()).await?;
    let concurrent_started = Instant::now();
    let concurrent = supervisor
        .create(OwnerId::new("startup-73-b"), mock())
        .await;
    assert!(
        matches!(concurrent, Err(SeatError::CapacityExhausted)),
        "full capacity must refuse at once: {concurrent:?}"
    );
    assert!(
        concurrent_started.elapsed() < TIMER_TOLERANCE,
        "refusal must not wait"
    );
    let (first, elapsed) = first.await?;
    assert_eq!(startup_teardown(first), TeardownStatus::Confirmed);
    assert_c5(elapsed);
    assert_unregistered_then_ready(&supervisor, &owner).await;
    Ok(())
}

#[tokio::test]
async fn must_pass_backend_ready_within_deadline_is_created_ready_73() -> TestResult {
    let factory = factory(0, 0, Readback::DropProbe);
    let supervisor = supervisor(Arc::clone(&factory) as Arc<dyn SeatBackendFactory>).await?;
    assert_unregistered_then_ready(&supervisor, &OwnerId::new("startup-73")).await;
    assert!(!factory.abandoned.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn must_fail_fast_factory_error_keeps_its_type_and_frees_capacity_73() -> TestResult {
    let factory = factory(1, 0, Readback::DropProbe);
    let supervisor = supervisor(factory as Arc<dyn SeatBackendFactory>).await?;
    let owner = OwnerId::new("startup-73");
    let failed = supervisor.create(owner.clone(), mock()).await;
    assert!(
        matches!(failed, Err(SeatError::Backend(ref detail)) if detail.contains("synthetic #73")),
        "a factory error must pass through unchanged: {failed:?}"
    );
    assert_unregistered_then_ready(&supervisor, &owner).await;
    Ok(())
}

#[tokio::test]
async fn must_fail_unregistered_backend_stays_backend_unavailable_73() -> TestResult {
    let supervisor = SeatSupervisor::new(SeatSupervisorConfig {
        browser: BrowserRuntimeConfig {
            chromium_executable: PathBuf::from("unused-startup-73.exe"),
            runtime_root: PathBuf::from("unused-startup-73"),
            startup_timeout_ms: STARTUP_MS,
        },
        max_seats: 1,
        per_seat_queue_capacity: 1,
        request_timeout_ms: REQUEST_MS,
    })?;
    let missing = supervisor.create(OwnerId::new("startup-73"), mock()).await;
    assert!(
        matches!(
            missing,
            Err(SeatError::BackendUnavailable(BackendKind::Mock))
        ),
        "{missing:?}"
    );
    Ok(())
}
