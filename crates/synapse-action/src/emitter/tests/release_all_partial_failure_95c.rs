//! #95 second review: a failed secondary (`ViGEm`) drain must reach callers.
//! The early primary ack may not hide it from the `release_all` tool, the K1
//! hotkey, or the per-session input ledger.

use super::*;
use crate::{ActionError, ActionHandle, ReleaseAllCompletion};

/// `ViGEm` stand-in whose `ReleaseAll` neutralizes the recorded state but then
/// reports a failure, like a pad that could not be unplugged.
struct FailingReleaseBackend {
    inner: RecordingBackend,
}

impl ActionBackend for FailingReleaseBackend {
    fn execute(&self, action: &Action, state: &mut EmitState) -> Result<(), ActionError> {
        let result = self.inner.execute(action, state);
        if matches!(action, Action::ReleaseAll) {
            return Err(ActionError::BackendUnavailable {
                detail: "vigem pad neutralization failed (test)".to_owned(),
            });
        }
        result
    }
}

/// Production-shaped backends: hardware drain off, software primary, `ViGEm`
/// secondary drain behind `vigem`.
fn backends_with_vigem(vigem: Arc<dyn ActionBackend>) -> Backends {
    Backends::from_parts(
        Arc::new(RecordingBackend::new()),
        vigem,
        Arc::new(RecordingBackend::new()),
        false,
    )
}

fn failing_vigem() -> Arc<dyn ActionBackend> {
    Arc::new(FailingReleaseBackend {
        inner: RecordingBackend::new(),
    })
}

/// Spawns the actor and holds pad 1 for `session`, returning that session's handle.
async fn actor_holding_pad(
    backends: Backends,
    session: &str,
) -> (
    ActionHandle,
    CancellationToken,
    JoinHandle<ActionStateSnapshot>,
) {
    let (handle, _snapshot_handle, emitter) = ActionEmitter::channel_with_backends(backends);
    let cancel = CancellationToken::new();
    let join = tokio::spawn(emitter.run(cancel.clone()));
    let session_handle = handle.with_session_id(Some(session.to_owned()));
    session_handle
        .execute(Action::PadReport {
            pad: 1,
            report: gamepad_report(PadButton::A),
        })
        .await
        .unwrap_or_else(|error| panic!("pad report should hold state: {error}"));
    (session_handle, cancel, join)
}

fn ledger_has_session(handle: &ActionHandle, session: &str) -> bool {
    handle
        .session_inputs_snapshot()
        .unwrap_or_else(|error| panic!("ledger should be readable: {error}"))
        .sessions
        .iter()
        .any(|entry| entry.session_id == session)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_execute_release_all_surfaces_failed_vigem_drain_95c() {
    // ReleaseAll through the handle bumps the process-global release epoch.
    let _epoch_guard = crate::test_support_epoch::RELEASE_EPOCH_TEST_LOCK
        .lock()
        .await;
    let (handle, cancel, join) =
        actor_holding_pad(backends_with_vigem(failing_vigem()), "s95c").await;
    assert!(ledger_has_session(&handle, "s95c"));

    let Err(error) = handle.execute(Action::ReleaseAll).await else {
        panic!("release_all must report the failed vigem drain, not the early primary ack");
    };
    assert_eq!(error.code(), error_codes::ACTION_BACKEND_UNAVAILABLE);
    assert!(
        error.detail().contains("vigem"),
        "detail={}",
        error.detail()
    );
    assert!(
        ledger_has_session(&handle, "s95c"),
        "a failed drain must not clear the per-session input record"
    );

    cancel.cancel();
    let _final_state = join_actor_or_panic(join).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_k1_reports_partial_failure_when_vigem_drain_fails_95c() {
    // ReleaseAll through the handle bumps the process-global release epoch.
    let _epoch_guard = crate::test_support_epoch::RELEASE_EPOCH_TEST_LOCK
        .lock()
        .await;
    let (handle, cancel, join) =
        actor_holding_pad(backends_with_vigem(failing_vigem()), "s95c").await;

    let k1_handle = handle.clone();
    let joined = tokio::task::spawn_blocking(move || {
        k1_handle.fire_release_all_blocking(Duration::from_secs(3), Duration::from_secs(3))
    })
    .await;
    let outcome = match joined {
        Ok(Ok(outcome)) => outcome,
        other => panic!("K1 should return an outcome: {other:?}"),
    };
    assert!(outcome.primary.is_ok(), "primary={:?}", outcome.primary);
    match &outcome.completion {
        ReleaseAllCompletion::Complete(Err(error)) => {
            assert_eq!(error.code(), error_codes::ACTION_BACKEND_UNAVAILABLE);
        }
        other => panic!("completion must carry the secondary failure, got {other:?}"),
    }
    assert!(outcome.into_result().is_err());
    assert!(
        ledger_has_session(&handle, "s95c"),
        "a failed drain must not clear the per-session input record"
    );

    cancel.cancel();
    let _final_state = join_actor_or_panic(join).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_execute_release_all_clears_session_record_when_every_drain_succeeds_95c() {
    // ReleaseAll through the handle bumps the process-global release epoch.
    let _epoch_guard = crate::test_support_epoch::RELEASE_EPOCH_TEST_LOCK
        .lock()
        .await;
    let healthy: Arc<dyn ActionBackend> = Arc::new(RecordingBackend::new());
    let (handle, cancel, join) = actor_holding_pad(backends_with_vigem(healthy), "s95c").await;
    assert!(ledger_has_session(&handle, "s95c"));

    handle
        .execute(Action::ReleaseAll)
        .await
        .unwrap_or_else(|error| panic!("healthy drains should succeed: {error}"));
    assert!(!ledger_has_session(&handle, "s95c"));

    cancel.cancel();
    let _final_state = join_actor_or_panic(join).await;
}
