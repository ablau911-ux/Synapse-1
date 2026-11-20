//! Regression for #209: repeated `session list` calls must share one ambient
//! process scan within the TTL instead of re-reading every process's PEB.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use super::ScanCache;

fn counting_scan(calls: &AtomicUsize) -> usize {
    calls.fetch_add(1, Ordering::SeqCst) + 1
}

#[test]
fn must_pass_second_call_within_ttl_reuses_the_scan() {
    let cache = ScanCache::new(Duration::from_mins(1));
    let calls = AtomicUsize::new(0);
    let first = cache.get(false, || counting_scan(&calls));
    let second = cache.get(false, || counting_scan(&calls));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*first, 1);
    assert_eq!(*second, 1);
}

#[test]
fn must_pass_expired_entry_is_rescanned() {
    let cache = ScanCache::new(Duration::ZERO);
    let calls = AtomicUsize::new(0);
    cache.get(false, || counting_scan(&calls));
    let second = cache.get(false, || counting_scan(&calls));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(*second, 2);
}

#[test]
fn must_fail_a_cwd_request_is_not_served_by_a_scan_taken_without_cwd() {
    let cache = ScanCache::new(Duration::from_secs(60));
    let calls = AtomicUsize::new(0);
    cache.get(false, || counting_scan(&calls));
    let with_cwd = cache.get(true, || counting_scan(&calls));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(*with_cwd, 2);
}

#[test]
fn must_pass_a_scan_taken_with_cwd_also_serves_a_request_without_cwd() {
    let cache = ScanCache::new(Duration::from_secs(60));
    let calls = AtomicUsize::new(0);
    cache.get(true, || counting_scan(&calls));
    cache.get(false, || counting_scan(&calls));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
