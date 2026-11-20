//! Regression tests for #214: a session torn down by a tool (`agent_kill`,
//! `adopt`) is only marked terminated; its rmcp session and worker used to stay
//! in the manager map for the 24 h keep-alive, counting as active and
//! suppressing failed-cleanup retries. The sweep must close such sessions.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::transport::streamable_http_server::session::{
    SessionManager as _, local::LocalSessionManager,
};
use tempfile::TempDir;

use super::close_terminated_http_sessions;
use crate::{http::shutdown_sessions::active_http_session_ids, test_support};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_tool_terminated_session_is_closed_in_the_rmcp_manager() {
    let _serial = test_support::lease_serial("terminated_close_214");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    let manager = LocalSessionManager::default();
    let (terminated_id, _terminated_transport) = manager.create_session().await.expect("session");
    let (live_id, _live_transport) = manager.create_session().await.expect("session");

    // Tool-initiated teardown (agent_kill / adopt path): marks the id
    // terminated but has no handle on the rmcp session.
    lifecycle
        .teardown_session(&terminated_id, "agent_kill")
        .await
        .expect("teardown");
    assert!(lifecycle.is_session_terminated(&terminated_id));
    assert!(
        active_http_session_ids(&manager)
            .await
            .contains(&*terminated_id),
        "precondition: the terminated session still sits in the rmcp map"
    );

    let closed = close_terminated_http_sessions(
        &lifecycle,
        &manager,
        &super::super::inflight_ids::InflightRequestIds::default(),
    )
    .await;

    assert_eq!(closed, vec![terminated_id.to_string()]);
    let active = active_http_session_ids(&manager).await;
    assert!(
        !active.contains(&*terminated_id),
        "terminated session must not count as active"
    );
    assert!(active.contains(&*live_id), "live session must stay active");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_nothing_is_closed_when_no_active_session_is_terminated() {
    let _serial = test_support::lease_serial("terminated_close_none_214");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    let manager = LocalSessionManager::default();
    let (live_id, _live_transport) = manager.create_session().await.expect("session");
    // Terminated but already gone from the manager (the DELETE path): no-op.
    lifecycle
        .teardown_session("already-gone-214", "http_delete")
        .await
        .expect("teardown");

    let closed = close_terminated_http_sessions(
        &lifecycle,
        &manager,
        &super::super::inflight_ids::InflightRequestIds::default(),
    )
    .await;

    assert!(closed.is_empty(), "unexpected closes: {closed:?}");
    assert!(active_http_session_ids(&manager).await.contains(&*live_id));
}
