//! Regression tests for #211: a raw-CDP tab the user already closed is a
//! successful cleanup (`AlreadyAbsent`), not a hard failure that pins the
//! session as `cleanup_failed` and re-runs the whole teardown every 30 s.

#![cfg(windows)]

use super::*;

fn raw_cdp_owner(target_id: &str) -> CdpTargetOwner {
    CdpTargetOwner {
        session_id: "session-211".to_owned(),
        window_hwnd: 0x2110,
        endpoint: "http://127.0.0.1:9".to_owned(),
        chrome_window_id: None,
        capture_window_hwnd: None,
        cdp_target_id: target_id.to_owned(),
        requested_url: "http://127.0.0.1/test".to_owned(),
        target_url: "http://127.0.0.1/test".to_owned(),
        created_at_unix_ms: 1,
    }
}

#[test]
fn must_pass_target_absent_before_close_maps_to_already_absent() {
    let owner = raw_cdp_owner("TARGET211");
    // The exact error `cdp_close_target` returns when `Target.getTargets` does
    // not list the target (the user closed the tab first).
    let error = synapse_a11y::cdp_close_target_absent_error("TARGET211");

    assert_eq!(
        raw_cdp_close_error_outcome(error, &owner),
        Ok(CdpCleanupCloseOutcome::AlreadyAbsent)
    );
}

#[test]
fn must_fail_unreachable_endpoint_still_maps_to_endpoint_unreachable() {
    let owner = raw_cdp_owner("TARGET211");
    let error = synapse_a11y::A11yError::CdpUnreachable {
        detail: "connection refused".to_owned(),
    };

    assert_eq!(
        raw_cdp_close_error_outcome(error, &owner),
        Ok(CdpCleanupCloseOutcome::EndpointUnreachable)
    );
}

#[test]
fn must_fail_a_real_close_failure_is_still_an_error() {
    let owner = raw_cdp_owner("TARGET211");
    let error = synapse_a11y::A11yError::CdpAxtreeFailed {
        detail: "Target.closeTarget(\"TARGET211\"): protocol error".to_owned(),
    };

    let outcome = raw_cdp_close_error_outcome(error, &owner);

    assert_eq!(
        outcome,
        Err("Chromium accessibility tree retrieval failed: \
             Target.closeTarget(\"TARGET211\"): protocol error"
            .to_owned()),
        "a real close failure must surface the full failure detail, never reclaim the row"
    );
}

#[test]
fn must_fail_the_absent_marker_is_not_matched_by_unrelated_axtree_failures() {
    assert!(!synapse_a11y::is_cdp_close_target_absent(
        &synapse_a11y::A11yError::CdpAxtreeFailed {
            detail: "Target.getTargets failed".to_owned(),
        }
    ));
    assert!(synapse_a11y::is_cdp_close_target_absent(
        &synapse_a11y::cdp_close_target_absent_error("T")
    ));
}
