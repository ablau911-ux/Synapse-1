//! #95 second review: `blocking_recv_with_timeout` is the K1 hotkey's wait on
//! the emitter acknowledgement. It must not depend on the tokio cooperative
//! budget of whatever task/thread happens to call it (panic hook on a worker).

use std::{
    task::Poll,
    thread,
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, oneshot};

use super::{AckWaitError, blocking_recv_with_timeout};

#[test]
fn must_pass_returns_value_that_was_already_sent() {
    let (tx, mut rx) = oneshot::channel();
    assert!(tx.send(7_u32).is_ok());
    let result = blocking_recv_with_timeout(&mut rx, Duration::from_millis(200));
    assert!(matches!(result, Ok(7)));
}

#[test]
fn must_pass_zero_timeout_still_returns_a_ready_value() {
    let (tx, mut rx) = oneshot::channel();
    assert!(tx.send(9_u32).is_ok());
    let result = blocking_recv_with_timeout(&mut rx, Duration::ZERO);
    assert!(matches!(result, Ok(9)));
}

#[test]
fn must_fail_reports_closed_when_sender_is_dropped() {
    let (tx, mut rx) = oneshot::channel::<u32>();
    drop(tx);
    let result = blocking_recv_with_timeout(&mut rx, Duration::from_millis(200));
    assert!(matches!(result, Err(AckWaitError::Closed)));
}

#[test]
fn must_fail_times_out_when_nothing_is_sent() {
    let (_tx, mut rx) = oneshot::channel::<u32>();
    let started = Instant::now();
    let result = blocking_recv_with_timeout(&mut rx, Duration::from_millis(60));
    assert!(matches!(result, Err(AckWaitError::TimedOut)));
    assert!(started.elapsed() >= Duration::from_millis(55));
}

#[test]
fn must_pass_wakes_promptly_when_another_thread_completes_the_sender() {
    let (tx, mut rx) = oneshot::channel();
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        tx.send(3_u32).is_ok()
    });
    let started = Instant::now();
    let result = blocking_recv_with_timeout(&mut rx, Duration::from_secs(5));
    assert!(matches!(result, Ok(3)));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(matches!(sender.join(), Ok(true)));
}

/// Regression: polling a tokio oneshot consumes the calling task's coop
/// budget. Inside a task whose budget is already exhausted the receiver
/// answers `Pending` and defers its wake-up, so the parked thread never saw an
/// already-delivered acknowledgement and reported a false timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_receives_ready_value_even_when_task_coop_budget_is_exhausted() {
    let task = tokio::spawn(std::future::poll_fn(|cx| {
        let (budget_tx, mut budget_rx) = mpsc::unbounded_channel::<u8>();
        for _ in 0..1024 {
            let _sent = budget_tx.send(0);
        }
        let mut drained = 0_u32;
        while drained < 1024 {
            match budget_rx.poll_recv(cx) {
                Poll::Ready(_) => drained += 1,
                Poll::Pending => break,
            }
        }
        assert!(
            drained < 1024,
            "test setup must exhaust the coop budget, drained={drained}"
        );
        let (tx, mut rx) = oneshot::channel();
        assert!(tx.send(11_u32).is_ok());
        Poll::Ready(blocking_recv_with_timeout(
            &mut rx,
            Duration::from_millis(300),
        ))
    }));
    match task.await {
        Ok(Ok(11)) => {}
        Ok(Ok(other)) => panic!("unexpected value {other}"),
        Ok(Err(AckWaitError::TimedOut)) => {
            panic!("false timeout: coop-exhausted task never saw the delivered ack")
        }
        Ok(Err(AckWaitError::Closed)) => panic!("unexpected closed"),
        Err(error) => panic!("wait task should join: {error}"),
    }
}
