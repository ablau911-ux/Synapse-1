//! #13: the HTTP layer must cap live MCP sessions. Sequential or concurrent
//! `initialize` calls beyond the cap are refused with a 503 diagnostic; capacity
//! returns when a session closes. Local callers stay unauthenticated by design:
//! this is a resource bound, not an access restriction.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{body::to_bytes, http::StatusCode};
use rmcp::transport::streamable_http_server::session::SessionManager as _;
use tower::ServiceExt as _;

use std::sync::Arc;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

use super::session_router_support::{Harness, INITIALIZE, PING, post_mcp};
use super::{SessionAdmission, SessionCleanupState};
use crate::http::session_admission::MAX_SESSIONS_ENV;

fn make(max: usize, create_sessions: bool) -> Harness {
    let manager = Arc::new(LocalSessionManager::default());
    let state =
        SessionCleanupState::request_state(Arc::clone(&manager), Arc::default(), Arc::default())
            .with_admission(SessionAdmission::new(max));
    Harness::build(manager, state, create_sessions)
}

#[tokio::test]
async fn must_fail_initialize_at_capacity_is_refused_with_503_readback() {
    let harness = make(2, false);
    let _s1 = harness.manager.create_session().await.unwrap();
    let _s2 = harness.manager.create_session().await.unwrap();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(None, "application/json", INITIALIZE))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["code"], "HTTP_SESSION_CAPACITY");
    assert_eq!(body["max_sessions"], 2);
    assert_eq!(body["active_sessions"], 2);
    assert_eq!(harness.reached(), 0);
}

#[tokio::test]
async fn must_pass_closing_a_session_releases_capacity() {
    let harness = make(1, false);
    let (id, _transport) = harness.manager.create_session().await.unwrap();
    let refused = harness
        .app
        .clone()
        .oneshot(post_mcp(None, "application/json", INITIALIZE))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    harness.manager.close_session(&id).await.unwrap();
    let admitted = harness
        .app
        .clone()
        .oneshot(post_mcp(None, "application/json", INITIALIZE))
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
    assert_eq!(harness.reached(), 1);
}

#[tokio::test]
async fn must_pass_requests_on_existing_sessions_are_not_capped() {
    let harness = make(1, false);
    let (id, _transport) = harness.manager.create_session().await.unwrap();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(Some(&*id), "application/json", PING))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.reached(), 1);
}

#[tokio::test]
async fn must_fail_concurrent_initializes_never_exceed_the_cap() {
    let harness = make(3, true);
    let calls: Vec<_> = (0..10)
        .map(|_| {
            let app = harness.app.clone();
            tokio::spawn(async move {
                app.oneshot(post_mcp(None, "application/json", INITIALIZE))
                    .await
                    .unwrap()
                    .status()
            })
        })
        .collect();
    let mut admitted = 0;
    let mut refused = 0;
    for call in calls {
        match call.await.unwrap() {
            StatusCode::OK => admitted += 1,
            StatusCode::SERVICE_UNAVAILABLE => refused += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!((admitted, refused), (3, 7));
    assert_eq!(harness.manager.sessions.read().await.len(), 3);
}

#[test]
fn must_fail_invalid_session_cap_values_are_rejected() {
    let zero = SessionAdmission::parse_max("0").unwrap_err();
    assert_eq!(
        format!("{zero:#}"),
        "max sessions must be greater than zero"
    );
    let garbage = SessionAdmission::parse_max("abc").unwrap_err();
    assert!(
        format!("{garbage:#}").contains("invalid integer \"abc\""),
        "{garbage:#}"
    );
    let empty = SessionAdmission::parse_max("  ").unwrap_err();
    assert!(
        format!("{empty:#}").contains("invalid integer \"\""),
        "{empty:#}"
    );
}

#[test]
fn must_pass_padded_session_cap_is_trimmed() {
    assert_eq!(SessionAdmission::parse_max(" 16 ").unwrap(), 16);
}

static MAX_SESSIONS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `body` with `MAX_SESSIONS_ENV` set to `value`, restoring it afterwards.
fn with_max_sessions_env(value: &str, body: impl FnOnce()) {
    let _lock = MAX_SESSIONS_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved = std::env::var_os(MAX_SESSIONS_ENV);
    // SAFETY: MAX_SESSIONS_ENV_LOCK serializes this file's env mutation and
    // set_var is always safe on Windows.
    unsafe { std::env::set_var(MAX_SESSIONS_ENV, value) };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    // SAFETY: as above.
    unsafe {
        match saved {
            Some(saved) => std::env::set_var(MAX_SESSIONS_ENV, saved),
            None => std::env::remove_var(MAX_SESSIONS_ENV),
        }
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn must_fail_invalid_env_cap_keeps_default() {
    let default_max = SessionAdmission::default().max_sessions();
    for raw in ["0", "abc", "-5", ""] {
        with_max_sessions_env(raw, || {
            let admission = SessionAdmission::from_env()
                .unwrap_or_else(|error| panic!("{raw:?} must not stop the daemon: {error:#}"));
            assert_eq!(admission.max_sessions(), default_max, "env {raw:?}");
        });
    }
}

#[test]
fn must_pass_valid_env_cap_is_applied() {
    with_max_sessions_env(" 7 ", || {
        let admission = SessionAdmission::from_env().unwrap();
        assert_eq!(admission.max_sessions(), 7);
    });
}
