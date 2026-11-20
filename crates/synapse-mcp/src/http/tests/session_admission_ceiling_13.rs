//! #13 (raid 2026-10-15): the session cap needs a hard upper bound, and every
//! refused initialize must be counted so `/health` can read it back. Before the
//! fix `parse_max` accepted any positive `usize` and the rejection counter had
//! no accessor (only a log line).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::http::StatusCode;
use rmcp::transport::streamable_http_server::session::{
    SessionManager as _, local::LocalSessionManager,
};
use tower::ServiceExt as _;

use super::session_router_support::{Harness, INITIALIZE, post_mcp};
use super::{SessionAdmission, SessionCleanupState};

/// Contract: issue #13 comment 1, "validated range 1..=4096".
const HARD_MAX_SESSIONS: usize = 4096;

/// Harness whose admission is shared with the caller for counter readback.
fn make(max: usize, create_sessions: bool) -> (Harness, SessionAdmission) {
    let manager = Arc::new(LocalSessionManager::default());
    let admission = SessionAdmission::new(max);
    let state =
        SessionCleanupState::request_state(Arc::clone(&manager), Arc::default(), Arc::default())
            .with_admission(admission.clone());
    (Harness::build(manager, state, create_sessions), admission)
}

#[test]
fn must_fail_session_cap_above_the_hard_ceiling_is_rejected() {
    let above = (HARD_MAX_SESSIONS + 1).to_string();
    let error = SessionAdmission::parse_max(&above).unwrap_err();
    assert!(
        format!("{error:#}").contains("4096"),
        "the refusal must name the 4096 ceiling: {error:#}"
    );
    let huge = SessionAdmission::parse_max(&usize::MAX.to_string()).unwrap_err();
    assert!(format!("{huge:#}").contains("4096"), "{huge:#}");
}

#[test]
fn must_pass_session_cap_at_the_hard_ceiling_and_floor_is_accepted() {
    assert_eq!(
        SessionAdmission::parse_max("4096").unwrap(),
        HARD_MAX_SESSIONS
    );
    assert_eq!(SessionAdmission::parse_max("1").unwrap(), 1);
}

#[tokio::test]
async fn must_fail_refused_initializes_are_counted_and_create_no_session() {
    let (harness, admission) = make(1, false);
    let (_id, _transport) = harness.manager.create_session().await.unwrap();
    assert_eq!(
        harness.manager.sessions.read().await.len(),
        1,
        "precondition"
    );
    assert_eq!(admission.rejected_total(), 0, "precondition: fresh counter");

    for _ in 0..3 {
        let refused = harness
            .app
            .clone()
            .oneshot(post_mcp(None, "application/json", INITIALIZE))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    assert_eq!(admission.rejected_total(), 3, "one count per refusal");
    assert_eq!(harness.manager.sessions.read().await.len(), 1);
    assert_eq!(harness.reached(), 0, "a refusal never reaches rmcp");
}

#[tokio::test]
async fn must_pass_admitted_initializes_are_not_counted_as_rejections() {
    let (harness, admission) = make(2, true);
    for _ in 0..2 {
        let admitted = harness
            .app
            .clone()
            .oneshot(post_mcp(None, "application/json", INITIALIZE))
            .await
            .unwrap();
        assert_eq!(admitted.status(), StatusCode::OK);
    }
    assert_eq!(harness.manager.sessions.read().await.len(), 2);
    assert_eq!(admission.rejected_total(), 0);
}
