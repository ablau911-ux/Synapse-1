//! #187 regression: the act_click real-foreground SendInput tier must be gated.
//! A normal profile may reach it only through a claimed route (claim + session
//! target + OS foreground + point inside the window); break_glass /
//! full_capability keep the sanctioned raw tier. A denied policy must refuse
//! before any lease is taken or input is sent.

use synapse_core::{Point, Rect, error_codes};

use super::foreground_gate::{
    ClaimFacts, ForegroundClickAuth, authorize_foreground_click, verify_claimed_click_foreground,
};
use super::{ForegroundClickPolicy, acquire_click_foreground_lease, click_error_data_code};

const ROOT: i64 = 0x1000;

fn facts(target_root: Option<i64>, claim_held: bool, session_target_matches: bool) -> ClaimFacts {
    ClaimFacts {
        target_root,
        claim_held,
        session_target_matches,
    }
}

fn window() -> Rect {
    Rect {
        x: 100,
        y: 100,
        w: 800,
        h: 600,
    }
}

#[test]
fn must_pass_foreground_capable_profile_is_authorized_without_a_claim() {
    let auth = authorize_foreground_click(true, &facts(None, false, false))
        .expect("break_glass / full_capability keep the raw foreground tier");
    assert_eq!(auth, ForegroundClickAuth::Profile);
}

#[test]
fn must_fail_normal_profile_without_a_target_window_is_denied() {
    let error = authorize_foreground_click(false, &facts(None, false, false))
        .expect_err("a bare screen-coordinate SendInput click must not be sanctioned");
    assert_eq!(
        click_error_data_code(&error),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED)
    );
}

#[test]
fn must_fail_normal_profile_without_a_claim_is_denied() {
    let error = authorize_foreground_click(false, &facts(Some(ROOT), false, true))
        .expect_err("an unclaimed window must not receive SendInput");
    assert_eq!(
        click_error_data_code(&error),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED)
    );
}

#[test]
fn must_fail_normal_profile_claim_on_a_non_session_target_is_denied() {
    let error = authorize_foreground_click(false, &facts(Some(ROOT), true, false))
        .expect_err("the click target must be the session target");
    assert_eq!(
        click_error_data_code(&error),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED)
    );
}

#[test]
fn must_pass_normal_profile_claimed_session_target_is_authorized_for_that_root() {
    let auth = authorize_foreground_click(false, &facts(Some(ROOT), true, true))
        .expect("claim + session target is the claimed route");
    assert_eq!(auth, ForegroundClickAuth::Claimed { root_hwnd: ROOT });
}

#[test]
fn must_fail_claimed_click_requires_the_target_to_be_the_os_foreground() {
    let point = Some(Point { x: 200, y: 200 });
    assert!(verify_claimed_click_foreground(ROOT, Some(ROOT), Some(window()), point).is_ok());

    let behind = verify_claimed_click_foreground(ROOT, Some(0x2000), Some(window()), point)
        .expect_err("target behind another window must not receive SendInput");
    assert_eq!(
        click_error_data_code(&behind),
        Some(error_codes::FOREGROUND_ACTIVATION_REFUSED)
    );
    let none = verify_claimed_click_foreground(ROOT, None, None, point)
        .expect_err("no OS foreground window means the target is not foreground");
    assert_eq!(
        click_error_data_code(&none),
        Some(error_codes::FOREGROUND_ACTIVATION_REFUSED)
    );
}

#[test]
fn must_fail_claimed_click_point_must_be_inside_the_target_window() {
    let outside = Some(Point { x: 5, y: 5 });
    let error = verify_claimed_click_foreground(ROOT, Some(ROOT), Some(window()), outside)
        .expect_err("a point outside the foreground target window lands in another window");
    assert_eq!(
        click_error_data_code(&error),
        Some(error_codes::ACTION_TARGET_INVALID)
    );
    let unknown_bounds =
        verify_claimed_click_foreground(ROOT, Some(ROOT), None, Some(Point { x: 200, y: 200 }))
            .expect_err("unreadable window bounds cannot prove the point is inside");
    assert_eq!(
        click_error_data_code(&unknown_bounds),
        Some(error_codes::ACTION_TARGET_INVALID)
    );
}

#[test]
fn must_fail_denied_policy_refuses_before_taking_the_lease() {
    // The lease is process-global: serialise with every other lease test and
    // start from a cleared lease so "no holder" below is this call's result.
    let _serial = crate::test_support::lease_serial("denied_click_policy_187");
    assert!(!synapse_action::lease::status().held);
    let denial = authorize_foreground_click(false, &facts(None, false, false))
        .expect_err("denied for the test");
    let policy =
        ForegroundClickPolicy::authorized(Some("denied-click-session".to_owned()), Err(denial));
    let mut attempts = Vec::new();
    let error = acquire_click_foreground_lease(&policy, 50, &mut attempts, None)
        .expect_err("a denied policy must not reach the SendInput tier");
    assert_eq!(
        click_error_data_code(&error),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED)
    );
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].tier, "foreground");
    assert_eq!(attempts[0].status, "failed");
    let lease = synapse_action::lease::status();
    assert!(
        !lease.held,
        "a refused click must not take the foreground lease: {lease:?}"
    );
    assert_eq!(lease.owner_session_id, None);
}
