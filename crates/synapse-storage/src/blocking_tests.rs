//! Regression tests for #60: maintenance work must not stall the async worker.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use super::run_blocking;

// A single-threaded runtime has exactly one worker: if the job ran inline the
// ticker below could not make progress while it slept.
#[tokio::test(flavor = "current_thread")]
async fn must_fail_a_slow_job_does_not_stall_other_tasks_on_the_runtime() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let ticker = {
        let ticks = Arc::clone(&ticks);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                ticks.fetch_add(1, Ordering::SeqCst);
            }
        })
    };

    let started = Instant::now();
    let value = run_blocking("test_slow_job", || {
        std::thread::sleep(Duration::from_millis(300));
        7_u32
    })
    .await;
    let elapsed = started.elapsed();
    ticker.abort();

    assert_eq!(value, Some(7));
    assert!(elapsed >= Duration::from_millis(300));
    // An inline (stalling) run yields 0 ticks; >= 2 proves the runtime kept
    // polling without depending on the 15.6 ms Windows timer resolution.
    assert!(
        ticks.load(Ordering::SeqCst) >= 2,
        "ticker advanced only {} times while the job ran",
        ticks.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn must_pass_a_quick_job_returns_its_value() {
    assert_eq!(run_blocking("test_quick_job", || 42_u32).await, Some(42));
}

#[tokio::test]
async fn must_fail_a_panicking_job_is_reported_as_none_without_killing_the_caller() {
    let value: Option<u32> = run_blocking("test_panic_job", || panic!("synthetic job panic")).await;
    assert_eq!(value, None);
}
