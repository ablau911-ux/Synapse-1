//! Regression tests for #61: retired-but-stuck UIA worker threads are bounded.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::{DEFAULT_MAX_LIVE_UIA_WORKER_THREADS, WorkerThreadGuard, may_start_worker};

#[test]
fn must_fail_a_new_worker_is_refused_once_the_bound_is_reached() {
    assert!(may_start_worker(0, DEFAULT_MAX_LIVE_UIA_WORKER_THREADS));
    assert!(may_start_worker(
        DEFAULT_MAX_LIVE_UIA_WORKER_THREADS - 1,
        DEFAULT_MAX_LIVE_UIA_WORKER_THREADS
    ));
    assert!(!may_start_worker(
        DEFAULT_MAX_LIVE_UIA_WORKER_THREADS,
        DEFAULT_MAX_LIVE_UIA_WORKER_THREADS
    ));
    assert!(!may_start_worker(usize::MAX, 1));
}

#[test]
fn must_pass_the_live_count_tracks_exactly_the_guard_lifetime() {
    // A private counter: other tests start real UIA workers concurrently.
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let first = WorkerThreadGuard::register_in(&COUNTER);
    let second = WorkerThreadGuard::register_in(&COUNTER);
    assert_eq!(COUNTER.load(Ordering::SeqCst), 2);
    drop(first);
    assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
    drop(second);
    assert_eq!(COUNTER.load(Ordering::SeqCst), 0);
}

#[test]
fn must_pass_a_guard_moved_into_a_thread_is_released_when_the_thread_ends() {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let guard = WorkerThreadGuard::register_in(&COUNTER);
    assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
    let handle = std::thread::spawn(move || {
        let _alive = guard;
    });
    handle.join().expect("thread");
    assert_eq!(COUNTER.load(Ordering::SeqCst), 0);
}
