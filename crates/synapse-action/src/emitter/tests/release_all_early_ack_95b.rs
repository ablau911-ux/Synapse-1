//! #95 K1 timing gap: the hotkey (`fire_release_all_blocking`) must get the
//! PRIMARY (software) release acknowledgement without waiting for slow
//! secondary drains, while the combined result stays reachable (#95 second
//! review). Covers the production config (hardware drain off, `ViGEm` drain
//! only when pads are held) as well as the hardware-enabled config.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc as std_mpsc,
};
use std::thread;

use super::*;
use crate::{ActionError, ActionHandle, ReleaseAllCompletion, ReleaseAllOutcome};

/// Backend whose first `ReleaseAll` blocks until the test opens the gate.
struct GatedReleaseBackend {
    inner: RecordingBackend,
    release_entered: AtomicBool,
    release_finished: AtomicBool,
    gate: Mutex<Option<std_mpsc::Receiver<()>>>,
}

impl GatedReleaseBackend {
    fn new(gate: std_mpsc::Receiver<()>) -> Self {
        Self {
            inner: RecordingBackend::new(),
            release_entered: AtomicBool::new(false),
            release_finished: AtomicBool::new(false),
            gate: Mutex::new(Some(gate)),
        }
    }
}

impl ActionBackend for GatedReleaseBackend {
    fn execute(&self, action: &Action, state: &mut EmitState) -> Result<(), ActionError> {
        let is_release_all = matches!(action, Action::ReleaseAll);
        if is_release_all {
            self.release_entered.store(true, Ordering::SeqCst);
            let gate = self.gate.lock().ok().and_then(|mut gate| gate.take());
            if let Some(gate) = gate {
                let _gate_result = gate.recv_timeout(Duration::from_secs(10));
            }
        }
        let result = self.inner.execute(action, state);
        if is_release_all {
            self.release_finished.store(true, Ordering::SeqCst);
        }
        result
    }
}

async fn wait_until(flag: &AtomicBool, what: &str) {
    for _attempt in 0..500 {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn k1_fire(
    handle: &ActionHandle,
    primary: Duration,
    completion: Duration,
) -> ReleaseAllOutcome {
    let handle = handle.clone();
    let joined =
        tokio::task::spawn_blocking(move || handle.fire_release_all_blocking(primary, completion))
            .await;
    match joined {
        Ok(Ok(outcome)) => outcome,
        other => panic!("K1 release_all should return an outcome: {other:?}"),
    }
}

async fn hold_pad(handle: &ActionHandle) {
    handle
        .execute(Action::PadReport {
            pad: 1,
            report: gamepad_report(PadButton::A),
        })
        .await
        .unwrap_or_else(|error| panic!("pad report should hold state: {error}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_k1_primary_ack_arrives_before_slow_hardware_drain_finishes_95b() {
    // ReleaseAll through the handle bumps the process-global release epoch.
    let _epoch_guard = crate::test_support_epoch::RELEASE_EPOCH_TEST_LOCK
        .lock()
        .await;
    let (gate_tx, gate_rx) = std_mpsc::channel();
    let software: Arc<dyn ActionBackend> = Arc::new(RecordingBackend::new());
    let vigem: Arc<dyn ActionBackend> = Arc::new(RecordingBackend::new());
    let hardware = Arc::new(GatedReleaseBackend::new(gate_rx));
    let hardware_backend: Arc<dyn ActionBackend> = hardware.clone();
    // hardware_release_enabled = true forces the secondary hardware drain.
    let backends = Backends::from_parts(software, vigem, hardware_backend, true);
    let (handle, _snapshot_handle, emitter) = ActionEmitter::channel_with_backends(backends);
    let cancel = CancellationToken::new();
    let join = tokio::spawn(emitter.run(cancel.clone()));

    // Hardware stays blocked (up to 10 s) until the gate opens, so a primary
    // ack that waits for the secondary drain cannot arrive inside this window.
    let outcome = k1_fire(&handle, Duration::from_secs(3), Duration::ZERO).await;
    assert!(outcome.primary.is_ok(), "primary={:?}", outcome.primary);
    assert!(
        matches!(outcome.completion, ReleaseAllCompletion::Pending),
        "slow hardware drain must still be pending: {:?}",
        outcome.completion
    );

    wait_until(
        &hardware.release_entered,
        "hardware drain to start after the ack",
    )
    .await;
    assert!(
        !hardware.release_finished.load(Ordering::SeqCst),
        "hardware drain must still be running when the ack is observed"
    );

    gate_tx
        .send(())
        .unwrap_or_else(|error| panic!("gate should open: {error}"));
    wait_until(
        &hardware.release_finished,
        "hardware drain to finish after the gate opens",
    )
    .await;

    cancel.cancel();
    let final_state = join_actor_or_panic(join).await;
    assert!(final_state.held_keys.is_empty());
}

/// Production config: `Backends::production()` keeps the hardware drain off, so
/// the only secondary drain is `ViGEm`, and only when a pad is held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_k1_primary_ack_arrives_before_slow_vigem_drain_in_production_config_95b() {
    // ReleaseAll through the handle bumps the process-global release epoch.
    let _epoch_guard = crate::test_support_epoch::RELEASE_EPOCH_TEST_LOCK
        .lock()
        .await;
    let (gate_tx, gate_rx) = std_mpsc::channel();
    let software: Arc<dyn ActionBackend> = Arc::new(RecordingBackend::new());
    let vigem = Arc::new(GatedReleaseBackend::new(gate_rx));
    let vigem_backend: Arc<dyn ActionBackend> = vigem.clone();
    let hardware: Arc<dyn ActionBackend> = Arc::new(RecordingBackend::new());
    let backends = Backends::from_parts(software, vigem_backend, hardware, false);
    let (handle, _snapshot_handle, emitter) = ActionEmitter::channel_with_backends(backends);
    let cancel = CancellationToken::new();
    let join = tokio::spawn(emitter.run(cancel.clone()));
    hold_pad(&handle).await;

    let outcome = k1_fire(&handle, Duration::from_secs(3), Duration::ZERO).await;
    assert!(outcome.primary.is_ok(), "primary={:?}", outcome.primary);
    assert!(
        matches!(outcome.completion, ReleaseAllCompletion::Pending),
        "slow vigem drain must still be pending: {:?}",
        outcome.completion
    );
    wait_until(&vigem.release_entered, "vigem drain to start after the ack").await;
    assert!(!vigem.release_finished.load(Ordering::SeqCst));

    gate_tx
        .send(())
        .unwrap_or_else(|error| panic!("gate should open: {error}"));
    wait_until(&vigem.release_finished, "vigem drain to finish").await;

    cancel.cancel();
    let final_state = join_actor_or_panic(join).await;
    assert!(final_state.pad_state.is_empty());
}

/// K1 waits (bounded) for the combined result after the early ack.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_k1_reports_complete_ok_when_secondary_drain_finishes_within_wait_95b() {
    // ReleaseAll through the handle bumps the process-global release epoch.
    let _epoch_guard = crate::test_support_epoch::RELEASE_EPOCH_TEST_LOCK
        .lock()
        .await;
    let (gate_tx, gate_rx) = std_mpsc::channel();
    let software: Arc<dyn ActionBackend> = Arc::new(RecordingBackend::new());
    let vigem = Arc::new(GatedReleaseBackend::new(gate_rx));
    let vigem_backend: Arc<dyn ActionBackend> = vigem.clone();
    let hardware: Arc<dyn ActionBackend> = Arc::new(RecordingBackend::new());
    let backends = Backends::from_parts(software, vigem_backend, hardware, false);
    let (handle, _snapshot_handle, emitter) = ActionEmitter::channel_with_backends(backends);
    let cancel = CancellationToken::new();
    let join = tokio::spawn(emitter.run(cancel.clone()));
    hold_pad(&handle).await;

    let opener = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        gate_tx.send(()).is_ok()
    });
    let outcome = k1_fire(&handle, Duration::from_secs(3), Duration::from_secs(5)).await;
    assert!(outcome.primary.is_ok());
    assert!(
        matches!(outcome.completion, ReleaseAllCompletion::Complete(Ok(()))),
        "completion={:?}",
        outcome.completion
    );
    assert!(matches!(opener.join(), Ok(true)));

    cancel.cancel();
    let _final_state = join_actor_or_panic(join).await;
}
