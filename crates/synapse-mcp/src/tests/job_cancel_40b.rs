//! #40 follow-up: cancelling a durable shell job must terminate the whole tree even
//! when one member cannot be killed.
//!
//! The deployed review saw `taskkill /F` fail on the job's `conhost.exe` child; the
//! termination order is deepest-first, so the loop stopped there and never reached the
//! job's root process, which kept running. Every target must be attempted and the first
//! error still reported.

use super::*;

fn start_times(pids: &[u32]) -> BTreeMap<u32, u64> {
    pids.iter()
        .map(|pid| (*pid, 1_000 + u64::from(*pid)))
        .collect()
}

#[test]
fn must_fail_failed_child_kill_does_not_skip_the_root_40b() {
    // conhost.exe (10620) is ordered before its python.exe parent (6816).
    let order = [10620_u32, 6816];
    let mut attempted = Vec::new();
    let error = terminate_in_order(&order, &start_times(&order), |pid, _start| {
        attempted.push(pid);
        if pid == 10620 {
            Err("taskkill /F failed for pid 10620: status=exit code: 1".to_owned())
        } else {
            Ok(())
        }
    });
    assert_eq!(
        attempted,
        vec![10620, 6816],
        "the root must still be killed"
    );
    assert_eq!(
        error.as_deref(),
        Some("taskkill /F failed for pid 10620: status=exit code: 1")
    );
}

#[test]
fn must_fail_first_error_is_kept_when_several_kills_fail_40b() {
    let order = [3_u32, 2, 1];
    let mut attempted = Vec::new();
    let error = terminate_in_order(&order, &start_times(&order), |pid, _start| {
        attempted.push(pid);
        Err(format!("fail {pid}"))
    });
    assert_eq!(attempted, vec![3, 2, 1]);
    assert_eq!(error.as_deref(), Some("fail 3"));
}

#[test]
fn must_pass_pids_without_a_start_time_are_skipped_and_success_has_no_error_40b() {
    let order = [5_u32, 6, 7];
    let times = start_times(&[5, 7]);
    let mut attempted = Vec::new();
    let error = terminate_in_order(&order, &times, |pid, start| {
        attempted.push((pid, start));
        Ok(())
    });
    assert_eq!(attempted, vec![(5, 1_005), (7, 1_007)]);
    assert_eq!(error, None);
}
