//! #28: syntactically invalid JSON on `POST /mcp` must be answered with 400 at
//! the session boundary, not forwarded to rmcp (whose deserialize failure
//! surfaces as 415). Real media-type failures keep rmcp's 415.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt as _;

use super::session_router_support::{Harness, INITIALIZE, post_mcp};

#[tokio::test]
async fn must_fail_malformed_json_without_session_is_400_and_never_reaches_rmcp() {
    let harness = Harness::new();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(None, "application/json", "{not json"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["error"]["code"], -32700);
    assert_eq!(harness.reached(), 0);
}

#[tokio::test]
async fn must_fail_malformed_json_with_session_header_is_400() {
    let harness = Harness::new();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(Some("any-session"), "application/json", "{\"a\":"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["error"]["code"], -32700);
    assert_eq!(body["error"]["data"]["reason"], "malformed_json");
    assert_eq!(harness.reached(), 0);
}

#[tokio::test]
async fn must_fail_empty_json_body_is_400() {
    let harness = Harness::new();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(None, "application/json; charset=utf-8", ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["error"]["code"], -32700);
    assert_eq!(body["error"]["data"]["reason"], "malformed_json");
    assert_eq!(harness.reached(), 0);
}

#[tokio::test]
async fn must_pass_wrong_media_type_passes_through_to_rmcp() {
    // Not our parse-error case: rmcp owns the 415 for a non-JSON content type,
    // so the middleware forwards it untouched (the harness stub answers 200).
    let harness = Harness::new();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(None, "text/plain", "{not json"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.reached(), 1);
}

#[tokio::test]
async fn must_pass_valid_initialize_still_reaches_the_service() {
    let harness = Harness::new();
    let response = harness
        .app
        .clone()
        .oneshot(post_mcp(None, "application/json", INITIALIZE))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.reached(), 1);
}
