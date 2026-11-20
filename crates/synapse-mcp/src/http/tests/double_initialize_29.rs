//! #29: a repeated `initialize` on a session that is already live must be
//! rejected so the negotiated protocol of that session is preserved.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{body::to_bytes, http::StatusCode};
use rmcp::transport::streamable_http_server::session::SessionManager as _;
use tower::ServiceExt as _;

use super::session_router_support::{Harness, INITIALIZE, PING, post_mcp};

#[tokio::test]
async fn must_fail_initialize_on_live_session_is_rejected_before_rmcp() {
    let harness = Harness::new();
    let (id, _transport) = harness.manager.create_session().await.unwrap();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(Some(&*id), "application/json", INITIALIZE))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["id"], 1);
    assert_eq!(body["error"]["code"], -32600);
    assert_eq!(
        body["error"]["data"]["reason"],
        "session_already_initialized"
    );
    assert_eq!(harness.reached(), 0);
}

#[tokio::test]
async fn must_fail_batch_containing_initialize_on_live_session_is_rejected() {
    let harness = Harness::new();
    let (id, _transport) = harness.manager.create_session().await.unwrap();
    let batch = format!("[{PING},{INITIALIZE}]");
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(Some(&*id), "application/json", &batch))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    // A batch has no single id.
    assert_eq!(body["id"], serde_json::Value::Null);
    assert_eq!(body["error"]["code"], -32600);
    assert_eq!(
        body["error"]["data"]["reason"],
        "session_already_initialized"
    );
    assert_eq!(body["error"]["data"]["session_id"], &*id);
    assert_eq!(harness.reached(), 0);
}

#[tokio::test]
async fn must_pass_ordinary_request_on_live_session_still_passes() {
    let harness = Harness::new();
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
async fn must_pass_initialize_on_unknown_session_is_left_to_rmcp() {
    // Unknown/expired ids keep the existing 404 recovery path inside rmcp.
    let harness = Harness::new();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(
            Some("never-created"),
            "application/json",
            INITIALIZE,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.reached(), 1);
}
