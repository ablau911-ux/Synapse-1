//! Regression tests for #122 (re-review): a start whose capture worker ended
//! must tear the capture down even when another start adopted it (a permanent
//! joiner must not suppress cleanup), and `status` must report the actual
//! worker state instead of `running` for a dead worker.

use super::super::livecapture_ownership::failed_start_should_stop;
use crate::m1::capture_status_label;

#[test]
fn must_fail_dead_worker_is_cleaned_up_even_when_another_start_adopted_it() {
    // creator, joiners present, worker ended
    assert!(failed_start_should_stop(true, true, false));
    // an adopter whose start failed on the same dead worker
    assert!(failed_start_should_stop(false, true, false));
    assert!(failed_start_should_stop(false, false, false));
}

#[test]
fn must_fail_sole_creator_with_dead_worker_stops() {
    // creator, no joiners, worker ended: the last truth-table row.
    assert!(failed_start_should_stop(true, false, false));
}

#[test]
fn must_pass_live_worker_keeps_adopted_capture_but_creator_alone_stops_it() {
    // timeout on a healthy worker that someone else adopted: keep it (#110)
    assert!(!failed_start_should_stop(true, true, true));
    // an adopter never stops a healthy shared capture
    assert!(!failed_start_should_stop(false, false, true));
    assert!(!failed_start_should_stop(false, true, true));
    // sole creator, nobody adopted, still no frame: stop (#110)
    assert!(failed_start_should_stop(true, false, true));
}

#[test]
fn must_pass_status_label_reflects_actual_worker_state() {
    assert_eq!(capture_status_label(true), "running");
    assert_eq!(capture_status_label(false), "stopped");
}
