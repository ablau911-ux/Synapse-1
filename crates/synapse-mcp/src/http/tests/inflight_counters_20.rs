//! #20 (raid 2026-10-15): the in-flight registry must expose how many requests
//! are being answered (total and per session) and how many duplicate in-flight
//! ids were refused, so `/health` can report them. Before the fix the registry
//! only answered `has_session` and duplicates were logged, never counted.
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
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio::sync::Notify;
use tower::ServiceExt as _;

use super::{SessionCleanupState, require_mcp_session};
use crate::http::inflight_ids::InflightRequestIds;

const PROMPT: Duration = Duration::from_secs(2);

/// Router whose handler holds every request until `release`; returns the
/// middleware's own in-flight registry for readback.
fn held_app(release: Arc<Notify>, entered: Arc<AtomicUsize>) -> (Router, InflightRequestIds) {
    let state = SessionCleanupState::request_state(
        Arc::new(LocalSessionManager::default()),
        Arc::default(),
        Arc::default(),
    );
    let registry = state.inflight_ids();
    let app = Router::new()
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
        .layer(middleware::from_fn_with_state(state, require_mcp_session));
    (app, registry)
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

/// Polls `read` until it equals `want`, failing after PROMPT.
async fn poll_eq(what: &str, want: usize, read: impl Fn() -> usize) {
    tokio::time::timeout(PROMPT, async {
        while read() != want {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}: expected {want}, saw {}", read()));
}

#[tokio::test]
async fn must_pass_inflight_counts_track_held_requests_per_session() {
    let release = Arc::new(Notify::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let (app, registry) = held_app(Arc::clone(&release), Arc::clone(&entered));
    assert_eq!(registry.inflight_count(), 0, "precondition");

    let held: Vec<_> = [("s20", 1), ("s20", 2), ("other", 1)]
        .into_iter()
        .map(|(session, id)| tokio::spawn(app.clone().oneshot(ping(session, id))))
        .collect();
    poll_eq("entered", 3, || entered.load(Ordering::SeqCst)).await;

    assert_eq!(registry.inflight_count(), 3);
    assert_eq!(registry.inflight_count_for_session("s20"), 2);
    assert_eq!(registry.inflight_count_for_session("other"), 1);
    assert_eq!(registry.inflight_count_for_session("absent"), 0);
    assert_eq!(registry.duplicate_rejections_total(), 0);

    release.notify_waiters();
    for pending in held {
        let response = tokio::time::timeout(PROMPT, pending)
            .await
            .expect("released request finishes")
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        drop(to_bytes(response.into_body(), 1024).await.unwrap());
    }
    poll_eq("in-flight after completion", 0, || {
        registry.inflight_count()
    })
    .await;
    assert_eq!(registry.duplicate_rejections_total(), 0);
}

#[tokio::test]
async fn must_fail_duplicate_inflight_id_is_counted_without_changing_the_inflight_count() {
    let release = Arc::new(Notify::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let (app, registry) = held_app(Arc::clone(&release), Arc::clone(&entered));
    let original = tokio::spawn(app.clone().oneshot(ping("s20", 7)));
    poll_eq("entered", 1, || entered.load(Ordering::SeqCst)).await;
    assert_eq!(registry.inflight_count(), 1, "precondition: one held claim");

    for _ in 0..2 {
        let dup = tokio::time::timeout(PROMPT, app.clone().oneshot(ping("s20", 7)))
            .await
            .expect("duplicate must not hang")
            .unwrap();
        assert_eq!(dup.status(), StatusCode::CONFLICT);
    }

    assert_eq!(registry.duplicate_rejections_total(), 2);
    assert_eq!(registry.inflight_count(), 1, "a refusal holds no claim");
    assert_eq!(registry.inflight_count_for_session("s20"), 1);
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "duplicates never forwarded"
    );

    release.notify_waiters();
    let first = original.await.unwrap().unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    drop(to_bytes(first.into_body(), 1024).await.unwrap());
    poll_eq("in-flight after completion", 0, || {
        registry.inflight_count()
    })
    .await;
    assert_eq!(
        registry.duplicate_rejections_total(),
        2,
        "counter is monotonic"
    );
}
