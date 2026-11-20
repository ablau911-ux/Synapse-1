//! #37: the session-admission decision must not be held across the downstream
//! initialize. One stalled initialize may keep its reserved slot, but it must
//! never block another client's initialize (admitted or refused) behind it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Router, body::to_bytes, http::StatusCode, middleware, routing::post};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio::sync::Notify;
use tower::ServiceExt as _;

use super::session_router_support::{INITIALIZE, post_mcp};
use super::{SessionAdmission, SessionCleanupState, require_mcp_session};

const PROMPT: Duration = Duration::from_secs(2);

/// The FIRST request to reach the inner handler stalls until `release`;
/// every later one answers at once. `entered` counts handler entries.
fn stalling_app(max: usize, release: Arc<Notify>, entered: Arc<AtomicUsize>) -> Router {
    let manager = Arc::new(LocalSessionManager::default());
    let state = SessionCleanupState::request_state(manager, Arc::default(), Arc::default())
        .with_admission(SessionAdmission::new(max));
    Router::new()
        .route(
            "/mcp",
            post(move || {
                let release = Arc::clone(&release);
                let entered = Arc::clone(&entered);
                async move {
                    let notified = release.notified();
                    if entered.fetch_add(1, Ordering::SeqCst) == 0 {
                        notified.await;
                    }
                    "ok"
                }
            }),
        )
        .layer(middleware::from_fn_with_state(state, require_mcp_session))
}

async fn wait_entered(entered: &AtomicUsize, count: usize) {
    tokio::time::timeout(PROMPT, async {
        while entered.load(Ordering::SeqCst) < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {count} handler entries"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_stalled_initialize_does_not_block_another_clients_initialize() {
    let release = Arc::new(Notify::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let app = stalling_app(4, Arc::clone(&release), Arc::clone(&entered));
    let stalled = tokio::spawn(
        app.clone()
            .oneshot(post_mcp(None, "application/json", INITIALIZE)),
    );
    wait_entered(&entered, 1).await;
    assert!(
        !stalled.is_finished(),
        "precondition: first initialize is stalled"
    );

    let second = tokio::time::timeout(
        PROMPT,
        app.clone()
            .oneshot(post_mcp(None, "application/json", INITIALIZE)),
    )
    .await
    .expect("a second client's initialize must not wait behind a stalled one")
    .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(entered.load(Ordering::SeqCst), 2);
    assert!(!stalled.is_finished(), "the stall is still in progress");

    release.notify_waiters();
    let first = tokio::time::timeout(PROMPT, stalled)
        .await
        .expect("released initialize finishes")
        .unwrap()
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_stalled_initialize_keeps_its_slot_and_overflow_is_refused_promptly() {
    let release = Arc::new(Notify::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let app = stalling_app(1, Arc::clone(&release), Arc::clone(&entered));
    let stalled = tokio::spawn(
        app.clone()
            .oneshot(post_mcp(None, "application/json", INITIALIZE)),
    );
    wait_entered(&entered, 1).await;

    let refused = tokio::time::timeout(
        PROMPT,
        app.clone()
            .oneshot(post_mcp(None, "application/json", INITIALIZE)),
    )
    .await
    .expect("an over-cap initialize must be refused, not queued behind the stall")
    .unwrap();
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(refused.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["code"], "HTTP_SESSION_CAPACITY");
    assert_eq!(body["pending_initializes"], 1);
    assert_eq!(body["active_sessions"], 0);
    assert_eq!(body["max_sessions"], 1);
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "refusal never reached rmcp"
    );

    release.notify_waiters();
    let first = tokio::time::timeout(PROMPT, stalled)
        .await
        .expect("released initialize finishes")
        .unwrap()
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let again = tokio::time::timeout(
        PROMPT,
        app.oneshot(post_mcp(None, "application/json", INITIALIZE)),
    )
    .await
    .expect("capacity returns once the stalled initialize ends")
    .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
}
