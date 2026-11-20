//! #20: a second same-session POST that reuses a JSON-RPC request id still in
//! flight must be rejected promptly instead of hanging forever inside rmcp.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    middleware,
    routing::post,
};
use tokio::sync::Notify;
use tower::ServiceExt as _;

use super::{SessionCleanupState, require_mcp_session};

fn router(release: Arc<Notify>) -> Router {
    router_counting(release, Arc::default())
}

/// Like `router`, but `entered` counts requests that reached the inner handler.
fn router_counting(release: Arc<Notify>, entered: Arc<AtomicUsize>) -> Router {
    let state = SessionCleanupState::request_state(
        Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ),
        Arc::default(),
        Arc::default(),
    );
    Router::new()
        .route(
            "/mcp",
            post(move || {
                let release = Arc::clone(&release);
                let entered = Arc::clone(&entered);
                async move {
                    let notified = release.notified();
                    entered.fetch_add(1, Ordering::SeqCst);
                    notified.await;
                    "ok"
                }
            }),
        )
        .layer(middleware::from_fn_with_state(state, require_mcp_session))
}

/// Waits (bounded) until `count` requests have entered the inner handler.
async fn wait_entered(entered: &AtomicUsize, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while entered.load(Ordering::SeqCst) < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "expected {count} requests in the inner handler, saw {}",
            entered.load(Ordering::SeqCst)
        )
    });
}

fn ping(session: &str, id: u64) -> Request<Body> {
    Request::post("/mcp")
        .header("Mcp-Session-Id", session)
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#
        )))
        .unwrap()
}

#[tokio::test]
async fn must_pass_sequential_reuse_of_completed_request_id_is_served() {
    let release = Arc::new(Notify::new());
    let app = router(Arc::clone(&release));
    for _ in 0..2 {
        // A stored permit lets the inner handler finish immediately.
        release.notify_one();
        let response =
            tokio::time::timeout(Duration::from_secs(2), app.clone().oneshot(ping("s20", 7)))
                .await
                .expect("completed id reuse must not hang")
                .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }
}

#[tokio::test]
async fn must_pass_distinct_inflight_ids_on_one_session_are_all_served() {
    let release = Arc::new(Notify::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let app = router_counting(Arc::clone(&release), Arc::clone(&entered));
    let first = tokio::spawn(app.clone().oneshot(ping("s20", 1)));
    let second = tokio::spawn(app.clone().oneshot(ping("s20", 2)));
    // Both distinct ids reached the inner service while both are in flight.
    wait_entered(&entered, 2).await;
    assert!(!first.is_finished() && !second.is_finished());

    release.notify_waiters();
    for pending in [first, second] {
        let response = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .expect("released request must finish")
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }
    assert_eq!(entered.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn must_fail_duplicate_inflight_request_id_is_rejected_promptly() {
    let release = Arc::new(Notify::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let app = router_counting(Arc::clone(&release), Arc::clone(&entered));
    let first = tokio::spawn(app.clone().oneshot(ping("s20", 7)));
    wait_entered(&entered, 1).await;

    let dup = tokio::time::timeout(Duration::from_secs(2), app.clone().oneshot(ping("s20", 7)))
        .await
        .expect("duplicate in-flight id must not hang")
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(dup.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], serde_json::json!(7));
    assert_eq!(body["error"]["code"], -32600);
    assert_eq!(
        body["error"]["data"]["reason"],
        "duplicate_inflight_request_id"
    );
    // The duplicate never reached the inner service.
    assert_eq!(entered.load(Ordering::SeqCst), 1);

    // Same id on another session and another id on this session are unaffected.
    let mut expected = 1;
    for (session, id) in [("other", 7), ("s20", 8)] {
        let pending = tokio::spawn(app.clone().oneshot(ping(session, id)));
        expected += 1;
        wait_entered(&entered, expected).await;
        pending.abort();
    }

    release.notify_waiters();
    let first = first.await.unwrap().unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    drop(to_bytes(first.into_body(), 1024).await.unwrap());

    // Once the original completed, its id is reusable.
    let again = tokio::spawn(app.oneshot(ping("s20", 7)));
    wait_entered(&entered, expected + 1).await;
    again.abort();
}
