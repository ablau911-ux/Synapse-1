//! Regression tests for #195: the queue-full diagnostic's real-cursor blocker
//! keeps moving the cursor after the tool returns, so its foreground input
//! lease must survive the guard drop (TTL covers the blocker) instead of being
//! released while the blocker is still running.

use crate::test_support;

const SESSION: &str = "issue195-session";

#[test]
fn must_pass_held_guard_leaves_the_lease_held_after_drop() {
    let _serial = test_support::lease_serial("queue_full_lease_195");
    let mut guard = crate::m2::acquire_foreground_input_lease_with_ttl(
        "action_diagnostic_queue_full_setup",
        Some(SESSION),
        5_000,
    )
    .expect("acquire lease");
    guard.hold_until_ttl_expiry("queue_full_blocker_still_running");
    drop(guard);

    let status = synapse_action::lease::status();
    assert!(status.held, "lease must outlive the guard: {status:?}");
    assert_eq!(status.owner_session_id.as_deref(), Some(SESSION));
}

#[test]
fn must_fail_ordinary_guard_does_not_keep_the_lease_after_drop() {
    let _serial = test_support::lease_serial("queue_full_lease_195");
    let guard =
        crate::m2::acquire_foreground_input_lease_with_ttl("act_click", Some(SESSION), 5_000)
            .expect("acquire lease");
    drop(guard);

    let status = synapse_action::lease::status();
    assert!(!status.held, "plain guard must release: {status:?}");
}
