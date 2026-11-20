//! #112 (re-review): the open-session snapshot is taken before the M1 lock, so
//! a session that initialised and claimed the capture AFTER the snapshot is
//! absent from it. That owner must still count as live; only an owner that
//! was already claimed when the snapshot was taken can be judged dead by it.
//
// SEAM NEEDED: these drive a private copy of src/m1/capture_owner.rs. The real
// `livecapture` stop/retarget gate needs a running WGC capture of a native
// window, which no daemon input can provide headless (no synthetic capture
// source); see livecapture_owner_112.rs.
#[allow(dead_code)]
#[path = "../src/m1/capture_owner.rs"]
mod capture_owner;

use capture_owner::CaptureOwnership;
use std::time::{Duration, Instant};

const A: &str = "session-a";
const B: &str = "session-b";
const CAP: &str = "capture-1";

#[test]
fn must_fail_owner_claimed_after_snapshot_is_live_even_if_missing_from_it() {
    let snapshot_taken_at = Instant::now();
    std::thread::sleep(Duration::from_millis(5));
    let mut ownership = CaptureOwnership::default();
    ownership.claim(CAP, Some(A));
    let conflict = ownership
        .check_mutation_with_snapshot(Some(CAP), Some(B), snapshot_taken_at, |_| false)
        .expect_err("owner registered after the snapshot must not be treated as terminated");
    assert_eq!(conflict.owner_session_id, A);
}

#[test]
fn must_pass_owner_claimed_before_snapshot_and_missing_from_it_is_terminated() {
    let mut ownership = CaptureOwnership::default();
    ownership.claim(CAP, Some(A));
    std::thread::sleep(Duration::from_millis(5));
    let snapshot_taken_at = Instant::now();
    assert!(
        ownership
            .check_mutation_with_snapshot(Some(CAP), Some(B), snapshot_taken_at, |_| false)
            .is_ok()
    );
}

#[test]
fn must_fail_owner_claimed_before_snapshot_and_present_in_it_stays_protected() {
    let mut ownership = CaptureOwnership::default();
    ownership.claim(CAP, Some(A));
    std::thread::sleep(Duration::from_millis(5));
    let snapshot_taken_at = Instant::now();
    let Err(conflict) =
        ownership.check_mutation_with_snapshot(Some(CAP), Some(B), snapshot_taken_at, |s| s == A)
    else {
        panic!("a live owner present in the snapshot must keep the capture protected");
    };
    assert_eq!(conflict.owner_session_id, A);
    assert_eq!(conflict.capture_id, CAP);
}

#[test]
fn must_pass_owner_and_unscoped_callers_are_unaffected_by_snapshot_age() {
    let snapshot_taken_at = Instant::now();
    let mut ownership = CaptureOwnership::default();
    ownership.claim(CAP, Some(A));
    assert!(
        ownership
            .check_mutation_with_snapshot(Some(CAP), Some(A), snapshot_taken_at, |_| false)
            .is_ok()
    );
    assert!(
        ownership
            .check_mutation_with_snapshot(Some(CAP), None, snapshot_taken_at, |_| false)
            .is_ok()
    );
}
