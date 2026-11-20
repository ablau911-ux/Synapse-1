//! #95 follow-up: cleanup fences grants and clears only its own acquisition.
use synapse_action::lease::{self, LeaseOutcome, LeaseStatus};

fn acquired(owner: &str) -> LeaseStatus {
    match lease::try_acquire(owner, lease::ttl_from_ms(30_000)) {
        LeaseOutcome::Acquired(status) => status,
        other => panic!("expected isolated acquisition: {other:?}"),
    }
}

#[test]
fn must_fail_nested_emergency_reservations_block_grants_until_last_drop() {
    let _lock = crate::test_support::lease_serial("fu2-95");
    assert!(!lease::emergency_cleanup_pending());
    let outer = lease::reserve_emergency_cleanup();
    let inner = lease::reserve_emergency_cleanup();
    drop(outer);
    assert!(lease::emergency_cleanup_pending());
    match lease::try_acquire("fu2-95", lease::ttl_from_ms(30_000)) {
        LeaseOutcome::Busy {
            holder,
            retry_after_ms,
        } => {
            assert!(!holder.held);
            assert_eq!(retry_after_ms, 100);
        }
        other => panic!("emergency fence did not refuse grant: {other:?}"),
    }
    assert!(!lease::status().held);
    drop(inner);
    assert!(!lease::emergency_cleanup_pending());
    assert_eq!(
        acquired("fu2-95").owner_session_id.as_deref(),
        Some("fu2-95")
    );
}

#[test]
fn must_fail_emergency_reservation_refuses_renew_and_handoff_without_identity_change() {
    let _lock = crate::test_support::lease_serial("fu2-95");
    let before = acquired("fu2-95");
    let _reservation = lease::reserve_emergency_cleanup();
    let renew = lease::renew("fu2-95", None).expect_err("fenced renewal");
    let handoff =
        lease::handoff("fu2-95", "peer95", lease::ttl_from_ms(30_000)).expect_err("fenced handoff");
    for error in [renew, handoff] {
        assert_eq!(error.code(), "ACTION_FOREGROUND_LEASE_BUSY");
    }
    let after = lease::status();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.owner_session_id, before.owner_session_id);
}

#[test]
fn must_fail_old_emergency_generation_cannot_clear_new_same_owner_acquisition() {
    let _lock = crate::test_support::lease_serial("fu2-95");
    let old = acquired(lease::OPERATOR_LEASE_OWNER_SESSION_ID);
    let _ = lease::force_clear("synthetic completed prior cleanup");
    let current = acquired(lease::OPERATOR_LEASE_OWNER_SESSION_ID);
    assert_ne!(old.generation, current.generation);
    assert_eq!(old.owner_session_id, current.owner_session_id);
    assert_eq!(
        lease::force_clear_if_generation(&old, "stale cleanup"),
        None
    );
    let after = lease::status();
    assert_eq!(after.generation, current.generation);
    assert_eq!(after.owner_session_id, current.owner_session_id);
    assert!(after.held);
}

#[test]
fn must_pass_matching_emergency_generation_clears_once() {
    let _lock = crate::test_support::lease_serial("fu2-95");
    let current = acquired(lease::OPERATOR_LEASE_OWNER_SESSION_ID);
    let removed =
        lease::force_clear_if_generation(&current, "matching cleanup").expect("exact match");
    assert_eq!(removed.generation, current.generation);
    assert_eq!(removed.owner_session_id, current.owner_session_id);
    assert!(!lease::status().held);
    assert_eq!(
        lease::force_clear_if_generation(&current, "duplicate cleanup"),
        None
    );
}
