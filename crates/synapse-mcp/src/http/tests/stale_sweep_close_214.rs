//! #214: the sweep closes a terminated session's rmcp connection only once its
//! own teardown and its in-flight requests are done.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::transport::streamable_http_server::session::{
    SessionManager as _, local::LocalSessionManager,
};
use tempfile::TempDir;

use super::super::inflight_ids::{ClaimOutcome, InflightRequestIds};
use super::{closable_terminated_sessions, close_terminated_http_sessions};
use crate::{http::shutdown_sessions::active_http_session_ids, test_support};

fn terminated() -> Vec<String> {
    vec![
        "s-done".to_owned(),
        "s-tearing-down".to_owned(),
        "s-answering".to_owned(),
    ]
}

#[test]
fn must_pass_a_terminated_idle_session_is_closed() {
    let closable = closable_terminated_sessions(terminated(), |_| false, |_| false);
    assert_eq!(closable, terminated());
}

#[test]
fn must_fail_a_session_still_tearing_down_or_answering_is_not_closed() {
    let closable = closable_terminated_sessions(
        terminated(),
        |session| session == "s-tearing-down",
        |session| session == "s-answering",
    );
    assert_eq!(closable, vec!["s-done".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_an_inflight_request_keeps_its_session_open_until_answered() {
    let _serial = test_support::lease_serial("inflight_close_214");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    let manager = LocalSessionManager::default();
    let (session_id, _transport) = manager.create_session().await.expect("session");
    lifecycle
        .teardown_session(&session_id, "agent_kill")
        .await
        .expect("teardown");
    assert!(lifecycle.is_session_terminated(&session_id));

    let inflight = InflightRequestIds::default();
    let ClaimOutcome::Held(claim) = inflight.claim(&session_id, "7") else {
        panic!("first claim must be held");
    };
    // Real sweep entry point: the session is still answering request 7.
    let closed = close_terminated_http_sessions(&lifecycle, &manager, &inflight).await;
    assert!(closed.is_empty(), "closed while answering: {closed:?}");
    assert!(
        active_http_session_ids(&manager)
            .await
            .contains(&*session_id),
        "a session with an in-flight request must stay in the rmcp map"
    );

    drop(claim);
    let closed = close_terminated_http_sessions(&lifecycle, &manager, &inflight).await;
    assert_eq!(closed, vec![session_id.to_string()]);
    assert!(
        !active_http_session_ids(&manager)
            .await
            .contains(&*session_id)
    );
}
