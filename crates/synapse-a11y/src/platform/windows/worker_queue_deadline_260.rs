//! #260: a UIA worker request's queue wait must not count against its
//! execution deadline. A request that expires while still queued is refused
//! (`phase=worker_busy`, typed queue timeout) and never retires the worker; a
//! late-admitted healthy request gets its full execution budget; only an
//! executing job that exceeds that budget retires the worker, and a fresh
//! worker then serves the next request.
//!
//! Every test takes the exclusive worker gate (it occupies the process-wide
//! worker on purpose); the occupant thread is admitted through the gate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use super::{read_current_worker_state, with_automation_operation, worker_test_gate};
use crate::{A11yError, UiaWorkerPhase};

const LONG: Duration = Duration::from_secs(5);

fn worker_thread_id(label: &str) -> u32 {
    with_automation_operation(label, LONG, |_| Ok(read_current_worker_state()))
        .expect("worker readback")
        .thread_id
}

/// Occupies the worker from a helper thread until `release` fires; returns
/// once the occupant's job is executing on the worker.
fn spawn_occupant<'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    gate: &'scope worker_test_gate::ExclusiveGate,
    release: mpsc::Receiver<()>,
) -> thread::ScopedJoinHandle<'scope, ()> {
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let handle = scope.spawn(move || {
        let _admitted = gate.admit_current_thread();
        with_automation_operation("occupant_260", LONG, move |_| {
            let _ = entered_tx.send(());
            let _ = release.recv_timeout(LONG);
            Ok(())
        })
        .expect("occupant job succeeds");
    });
    entered_rx
        .recv_timeout(LONG)
        .expect("occupant job is executing");
    handle
}

fn release_after<'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    delay: Duration,
    release: mpsc::SyncSender<()>,
) {
    scope.spawn(move || {
        thread::sleep(delay);
        let _ = release.send(());
    });
}

#[test]
fn must_fail_late_admitted_healthy_request_succeeds_and_does_not_retire_the_worker() {
    let gate = worker_test_gate::exclusive();
    let initial = worker_thread_id("baseline_260");
    let late = thread::scope(|scope| {
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let occupant = spawn_occupant(scope, &gate, release_rx);
        // Queued ~350 ms of a 600 ms budget, then a healthy 400 ms job: the
        // old shared deadline left ~250 ms and retired a healthy worker.
        release_after(scope, Duration::from_millis(350), release_tx);
        let budget = Duration::from_millis(600);
        let late = with_automation_operation("late_admitted_260", budget, |_| {
            thread::sleep(Duration::from_millis(400));
            Ok(read_current_worker_state())
        });
        occupant.join().expect("occupant thread");
        late
    })
    .unwrap_or_else(|error| panic!("late-admitted healthy request must succeed: {error}"));
    assert_eq!(
        late.thread_id, initial,
        "late request ran on the same worker"
    );
    assert_eq!(
        worker_thread_id("after_late_260"),
        initial,
        "a healthy late request must not retire the worker"
    );
}

#[test]
fn must_fail_request_expiring_in_queue_is_refused_as_queue_timeout_and_keeps_the_worker() {
    let gate = worker_test_gate::exclusive();
    let initial = worker_thread_id("baseline_queue_260");
    let refused = thread::scope(|scope| {
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let occupant = spawn_occupant(scope, &gate, release_rx);
        let budget = Duration::from_millis(50);
        let refused = with_automation_operation("queued_expiry_260", budget, |_| Ok(()));
        let _ = release_tx.send(());
        occupant.join().expect("occupant thread");
        refused
    })
    .expect_err("a request still queued at its deadline must be refused");
    assert!(
        matches!(
            refused,
            A11yError::UiaWorkerTimeout {
                phase: UiaWorkerPhase::WorkerBusy,
                ..
            }
        ),
        "expected a WorkerBusy queue timeout, got {refused:?}"
    );
    let detail = refused.to_string();
    assert!(detail.contains("phase=worker_busy"), "{detail}");
    assert!(detail.contains("queue_timeout=true"), "{detail}");
    assert_eq!(
        worker_thread_id("after_queue_expiry_260"),
        initial,
        "a queue timeout must not retire the worker"
    );
}

#[test]
fn must_pass_stuck_executing_job_still_retires_and_a_fresh_worker_serves_the_next_request() {
    let gate = worker_test_gate::exclusive();
    let initial = worker_thread_id("baseline_stuck_260");
    let (hung_release_tx, hung_release_rx) = mpsc::sync_channel::<()>(1);
    let started = Instant::now();
    let stuck = thread::scope(|scope| {
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let occupant = spawn_occupant(scope, &gate, release_rx);
        release_after(scope, Duration::from_millis(150), release_tx);
        let budget = Duration::from_millis(300);
        let stuck = with_automation_operation("stuck_after_queue_260", budget, move |_| {
            let _ = hung_release_rx.recv_timeout(LONG);
            Ok(())
        });
        occupant.join().expect("occupant thread");
        stuck
    })
    .expect_err("a job that exceeds its execution deadline must time out");
    let elapsed = started.elapsed();
    let _ = hung_release_tx.send(());
    assert!(
        matches!(
            stuck,
            A11yError::UiaWorkerTimeout {
                phase: UiaWorkerPhase::ReplyRecv,
                ..
            }
        ),
        "expected ReplyRecv, got {stuck:?}"
    );
    // ~150 ms queued + the full 300 ms execution budget from dispatch.
    assert!(
        elapsed >= Duration::from_millis(440),
        "execution deadline must start at dispatch, not at enqueue: {elapsed:?}"
    );
    assert_ne!(
        worker_thread_id("after_stuck_260"),
        initial,
        "a fresh worker must serve the next request after a real retirement"
    );
}
