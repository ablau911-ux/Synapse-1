//! #61: Production-path tests for `with_automation_operation` worker timeout
//! retirement, fresh worker recovery, and `phase=worker_limit` admission cap.

use std::{
    sync::{Arc, atomic::Ordering, mpsc},
    time::Duration,
};

use synapse_core::error_codes::A11Y_UIA_WORKER_TIMEOUT;

use super::{
    LIVE_UIA_WORKER_THREADS, ProcessUiaWorker, lock_worker_slot, max_live_uia_worker_threads,
    read_current_worker_state, with_automation_operation, worker_test_gate,
};
use crate::A11yError;

struct SaturatedWorkerStateGuard {
    previous_worker: Option<Arc<ProcessUiaWorker>>,
    added_threads: usize,
}

impl SaturatedWorkerStateGuard {
    fn enter() -> Self {
        let previous_worker = {
            let mut slot = lock_worker_slot().expect("lock worker slot");
            slot.take()
        };
        let max_threads = max_live_uia_worker_threads();
        LIVE_UIA_WORKER_THREADS.fetch_add(max_threads, Ordering::SeqCst);
        Self {
            previous_worker,
            added_threads: max_threads,
        }
    }
}

impl Drop for SaturatedWorkerStateGuard {
    fn drop(&mut self) {
        LIVE_UIA_WORKER_THREADS.fetch_sub(self.added_threads, Ordering::SeqCst);
        if let Ok(mut slot) = lock_worker_slot()
            && slot.is_none()
        {
            *slot = self.previous_worker.take();
        }
    }
}

#[test]
fn must_fail_with_automation_operation_refuses_new_worker_when_live_threads_reach_cap() {
    // Exclusive: the saturated slot must not fail other tests' UIA jobs.
    let _gate = worker_test_gate::exclusive();
    let _guard = SaturatedWorkerStateGuard::enter();
    let err = with_automation_operation(
        "saturated_worker_probe_61",
        Duration::from_millis(50),
        |_| Ok(()),
    )
    .expect_err("saturated worker pool must refuse starting another worker");

    assert!(
        matches!(err, A11yError::UiaWorkerTimeout { .. }),
        "expected UiaWorkerTimeout, got: {err:?}"
    );
    assert_eq!(err.code(), A11Y_UIA_WORKER_TIMEOUT);
    let detail = err.to_string();
    assert!(detail.contains("phase=worker_limit"), "got: {detail}");
}

#[test]
fn must_fail_timed_out_job_retires_worker_and_must_pass_subsequent_job_recovers_on_fresh_worker() {
    // Exclusive: the deliberate hang retires the shared worker, which would
    // fail any other test's UIA job queued behind it.
    let _gate = worker_test_gate::exclusive();
    let initial = with_automation_operation("baseline_worker_61", Duration::from_secs(5), |_| {
        Ok(read_current_worker_state())
    })
    .expect("baseline worker readback");

    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let timeout_err = with_automation_operation(
        "hanging_provider_call_61",
        Duration::from_millis(40),
        move |_| {
            let _ = entered_tx.send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            Ok(())
        },
    )
    .expect_err("hanging job must time out");

    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worker entered job");
    assert_eq!(timeout_err.code(), A11Y_UIA_WORKER_TIMEOUT);
    assert!(
        timeout_err.to_string().contains("phase=worker_reply_recv"),
        "got: {timeout_err}"
    );

    let _ = release_tx.send(());
    let recovered =
        with_automation_operation("recovered_worker_61", Duration::from_secs(5), |_| {
            Ok(read_current_worker_state())
        })
        .expect("subsequent job must succeed on a fresh worker");
    assert_ne!(
        recovered.thread_id, initial.thread_id,
        "retired worker must be replaced by a fresh worker thread"
    );
}
