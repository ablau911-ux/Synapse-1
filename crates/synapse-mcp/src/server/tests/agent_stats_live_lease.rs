//! Regression tests for #91: `agent stats` reports the authoritative live
//! lease state next to the journal-derived `held_open` figure.

use synapse_action::LeaseStatus;

use super::*;

#[test]
fn must_pass_unheld_lease_reports_not_held_with_no_owner() {
    let live = LiveLeaseState::from_status(&LeaseStatus::unheld());
    assert!(!live.held);
    assert_eq!(live.owner_session_id, None);
    assert_eq!(live.expires_in_ms, None);
}

#[test]
fn must_pass_held_lease_reports_owner_and_remaining_ttl() {
    let status = LeaseStatus {
        held: true,
        owner_session_id: Some("session-a".to_owned()),
        acquired_at_ms_ago: Some(10),
        renewed_at_ms_ago: Some(5),
        ttl_ms: Some(5_000),
        expires_in_ms: Some(4_995),
        generation: None,
    };
    let live = LiveLeaseState::from_status(&status);
    assert!(live.held);
    assert_eq!(live.owner_session_id.as_deref(), Some("session-a"));
    assert_eq!(live.expires_in_ms, Some(4_995));
}
