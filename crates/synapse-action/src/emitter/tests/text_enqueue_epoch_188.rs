//! #188 follow-up: the emitter hands the backend the operator release epoch
//! read when the action was ENQUEUED, so a text backend can stop a chunk for an
//! interrupt raised while it was still waiting in the queue.

use synapse_core::KeystrokeDynamics;

use super::*;
use crate::{ActionError, ActionHandle, test_support_epoch::RELEASE_EPOCH_TEST_LOCK};

#[derive(Default)]
struct EpochProbeBackend {
    seen: Mutex<Vec<(String, Option<u64>)>>,
}

impl ActionBackend for EpochProbeBackend {
    fn execute(&self, action: &Action, state: &mut EmitState) -> Result<(), ActionError> {
        if let Action::TypeText { text, .. } = action
            && let Ok(mut seen) = self.seen.lock()
        {
            seen.push((text.clone(), state.enqueued_release_epoch()));
        }
        Ok(())
    }
}

fn type_text(text: &str) -> Action {
    Action::TypeText {
        text: text.to_owned(),
        dynamics: KeystrokeDynamics::Burst,
        backend: Backend::Software,
    }
}

fn probe_emitter(probe: &Arc<EpochProbeBackend>) -> (ActionHandle, ActionEmitter) {
    let backend: Arc<dyn ActionBackend> = Arc::clone(probe) as Arc<dyn ActionBackend>;
    let (handle, _snapshots, emitter) = ActionEmitter::channel_with_backend(backend);
    (handle, emitter)
}

async fn wait_for_seen(probe: &EpochProbeBackend) -> Vec<(String, Option<u64>)> {
    for _ in 0..500 {
        let seen = probe
            .seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default();
        if !seen.is_empty() {
            return seen;
        }
        time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the text action never reached the backend");
}

#[tokio::test]
async fn must_fail_interrupt_after_enqueue_is_visible_to_the_text_backend() {
    let _guard = RELEASE_EPOCH_TEST_LOCK.lock().await;
    let probe = Arc::new(EpochProbeBackend::default());
    let (handle, emitter) = probe_emitter(&probe);
    // The epoch at enqueue time; the lock keeps every other test from bumping it.
    let enqueue_epoch = crate::operator_release_epoch();
    // Enqueue while the emitter is not yet running: the chunk waits in the queue.
    handle
        .try_execute(type_text("queued secret"))
        .unwrap_or_else(|error| panic!("enqueue failed: {error:?}"));
    // The foreground monitor raises the interrupt after enqueue, before start.
    crate::request_release_interrupt();
    let cancel = CancellationToken::new();
    let join = tokio::spawn(emitter.run(cancel.clone()));
    let seen = wait_for_seen(&probe).await;
    cancel.cancel();
    let _snapshot = join.await;

    assert_eq!(seen.len(), 1, "seen={seen:?}");
    let epoch = seen[0]
        .1
        .unwrap_or_else(|| panic!("the text backend must receive the enqueue epoch"));
    // Exactly the enqueue-time value: an emitter that re-reads the epoch at run
    // start would hand over `enqueue_epoch + 1` and hide the interrupt.
    assert_eq!(
        epoch, enqueue_epoch,
        "the backend must get the epoch read at enqueue, not at run start"
    );
    assert!(
        crate::operator_release_requested_since(epoch),
        "an interrupt raised after enqueue must be visible against the enqueue epoch"
    );
}

#[tokio::test]
async fn must_pass_text_reaches_the_backend_once_with_its_enqueue_epoch() {
    let _guard = RELEASE_EPOCH_TEST_LOCK.lock().await;
    let probe = Arc::new(EpochProbeBackend::default());
    let (handle, emitter) = probe_emitter(&probe);
    let cancel = CancellationToken::new();
    let join = tokio::spawn(emitter.run(cancel.clone()));
    let before = crate::operator_release_epoch();
    handle
        .execute(type_text("typed once"))
        .await
        .unwrap_or_else(|error| panic!("execute failed: {error:?}"));
    let seen = wait_for_seen(&probe).await;
    cancel.cancel();
    let _snapshot = join.await;

    assert_eq!(seen.len(), 1, "the text must be dispatched exactly once");
    assert_eq!(seen[0].0, "typed once");
    let epoch = seen[0]
        .1
        .unwrap_or_else(|| panic!("the text backend must receive the enqueue epoch"));
    assert_eq!(
        epoch, before,
        "no interrupt was raised, so the enqueue epoch is the current epoch"
    );
}

#[tokio::test]
async fn must_pass_try_execute_with_completion_reports_the_final_result() {
    let probe = Arc::new(EpochProbeBackend::default());
    let (handle, emitter) = probe_emitter(&probe);
    let completion = handle
        .try_execute_with_completion(type_text("watched"))
        .unwrap_or_else(|error| panic!("enqueue failed: {error:?}"));
    let cancel = CancellationToken::new();
    let join = tokio::spawn(emitter.run(cancel.clone()));
    let result = time::timeout(Duration::from_secs(5), completion)
        .await
        .unwrap_or_else(|_elapsed| panic!("completion never arrived"))
        .unwrap_or_else(|_closed| panic!("completion channel closed"));
    cancel.cancel();
    let _snapshot = join.await;

    assert!(result.is_ok(), "result={result:?}");
    assert_eq!(wait_for_seen(&probe).await.len(), 1);
}
