//! Regression tests for #205: an explicit `lease_acquire` that lands while the
//! same session's foreground action holds an auto-acquired lease must not be
//! undone when the action's guard drops, and a failed handoff's rollback must
//! not invent a lifecycle row the holder never had.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::ErrorData;
use synapse_storage::cf;
use tempfile::TempDir;

use super::lease_tools_test_support::acquire_lease_for_session;
use crate::test_support;

const SESSION: &str = "issue205-session";
const PEER: &str = "issue205-peer";
const LEASE_ROW_PREFIX: &[u8] = b"mcp/session-lease/v1/";

fn action_lease() -> crate::m2::ForegroundInputLeaseGuard {
    crate::m2::acquire_foreground_input_lease_with_ttl("act_click", Some(SESSION), 5_000)
        .expect("action lease")
}

#[test]
fn must_pass_explicit_acquire_over_an_action_lease_survives_the_guard_drop() {
    let _serial = test_support::lease_serial("lease_explicit_over_auto_205");
    let guard = action_lease();

    let response = acquire_lease_for_session(SESSION, 30_000).expect("explicit acquire");
    drop(guard);

    // Taking over an auto lease is a fresh explicit acquisition (journaled),
    // not a renewal of something the caller never explicitly held.
    assert_eq!(response.outcome, "acquired");
    let status = synapse_action::lease::status();
    assert!(
        status.held,
        "the explicit lease vanished when the action ended: {status:?}"
    );
    assert_eq!(status.owner_session_id.as_deref(), Some(SESSION));
}

#[test]
fn must_pass_a_second_explicit_acquire_is_a_plain_renewal() {
    let _serial = test_support::lease_serial("lease_explicit_twice_205");
    let first = acquire_lease_for_session(SESSION, 30_000).expect("first");
    let second = acquire_lease_for_session(SESSION, 30_000).expect("second");

    assert_eq!(first.outcome, "acquired");
    assert_eq!(second.outcome, "renewed");
}

fn lease_rows(db: &synapse_storage::Db) -> Vec<String> {
    db.scan_cf_prefix(cf::CF_SESSIONS, LEASE_ROW_PREFIX)
        .expect("scan lease rows")
        .into_iter()
        .map(|(key, _value)| String::from_utf8_lossy(&key).into_owned())
        .collect()
}

fn forward_handoff() {
    synapse_action::lease::handoff(SESSION, PEER, synapse_action::lease::ttl_from_ms(5_000))
        .expect("forward handoff");
}

#[tokio::test]
async fn must_fail_handoff_rollback_creates_no_row_for_a_holder_that_never_had_one() {
    let _serial = test_support::lease_serial("lease_handoff_rollback_205");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let db = service
        .m3_state_handle()
        .lock()
        .expect("m3 state")
        .ensure_storage()
        .expect("storage");
    // The holder's lease is action-acquired: it has no persisted row/event.
    let _guard = action_lease();
    let prior = synapse_action::lease::status();
    forward_handoff();

    service.rollback_handoff_after_persist_failure(
        SESSION,
        PEER,
        &prior,
        &ErrorData::internal_error("persist failed", None),
    );

    assert_eq!(
        synapse_action::lease::status().owner_session_id.as_deref(),
        Some(SESSION),
        "memory must be rolled back to the prior holder"
    );
    let rows = lease_rows(&db);
    assert!(
        rows.iter().all(|key| !key.ends_with(SESSION)),
        "rollback invented an unjournaled lease row: {rows:?}"
    );
}

#[tokio::test]
async fn must_pass_handoff_rollback_keeps_the_row_of_a_holder_that_had_one() {
    let _serial = test_support::lease_serial("lease_handoff_rollback_row_205");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let db = service
        .m3_state_handle()
        .lock()
        .expect("m3 state")
        .ensure_storage()
        .expect("storage");
    acquire_lease_for_session(SESSION, 5_000).expect("explicit acquire");
    let prior = synapse_action::lease::status();
    service
        .persist_session_lease_with_acquisition_event(SESSION, &prior)
        .expect("journaled row");
    forward_handoff();

    service.rollback_handoff_after_persist_failure(
        SESSION,
        PEER,
        &prior,
        &ErrorData::internal_error("persist failed", None),
    );

    let rows = lease_rows(&db);
    assert!(
        rows.iter().any(|key| key.ends_with(SESSION)),
        "the holder's journaled row must survive the rollback: {rows:?}"
    );
}
