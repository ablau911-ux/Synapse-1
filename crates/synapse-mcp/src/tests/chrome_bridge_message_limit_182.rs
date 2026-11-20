//! #182: an over-limit or undecodable `/chrome-debugger/native/message` body
//! gets a typed JSON error (not axum's plain-text 413), and the limit covers
//! replies far above axum's implicit 2 MiB default.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::DefaultBodyLimit,
    http::{Request, StatusCode, header},
    routing::post,
};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::{
    BRIDGE_MESSAGE_REJECTED, ChromeDebuggerBridgeError, MAX_BRIDGE_HTTP_MESSAGE_BYTES,
    RESULT_TOO_LARGE, http_message,
};

const TEST_LIMIT: usize = 4096;

fn app() -> Router {
    Router::new().route(
        "/m",
        post(http_message).layer(DefaultBodyLimit::max(TEST_LIMIT)),
    )
}

async fn post_body(body: String) -> (StatusCode, Value) {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/m")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "body must be JSON, got {error}: {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, value)
}

#[test]
fn must_pass_limit_matches_native_message_cap_and_exceeds_axum_default() {
    assert_eq!(MAX_BRIDGE_HTTP_MESSAGE_BYTES, 64 * 1024 * 1024);
    const { assert!(MAX_BRIDGE_HTTP_MESSAGE_BYTES > 2 * 1024 * 1024) };
}

#[tokio::test]
async fn must_fail_oversize_body_returns_json_413_with_typed_code() {
    let body = json!({
        "host_id": "h",
        "message": {"type": "response", "id": "c", "ok": true, "blob": "x".repeat(TEST_LIMIT * 2)}
    })
    .to_string();
    let (status, value) = post_body(body).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(value["ok"], json!(false));
    assert_eq!(value["code"], json!(BRIDGE_MESSAGE_REJECTED));
    assert_eq!(value["max_bytes"], json!(MAX_BRIDGE_HTTP_MESSAGE_BYTES));
}

#[tokio::test]
async fn must_fail_malformed_json_returns_typed_json_error() {
    let (status, value) = post_body("{not json".to_owned()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "syntax error is a 400");
    assert_eq!(value["ok"], json!(false));
    assert_eq!(value["code"], json!(BRIDGE_MESSAGE_REJECTED));
}

#[tokio::test]
async fn must_fail_structurally_invalid_body_returns_typed_json_error() {
    let (status, value) = post_body(json!({"host_id": 5}).to_string()).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "well-formed JSON with the wrong shape is a 422"
    );
    assert_eq!(value["ok"], json!(false));
    assert_eq!(value["code"], json!(BRIDGE_MESSAGE_REJECTED));
}

#[test]
fn must_pass_extension_result_too_large_code_is_preserved_for_callers() {
    let error = ChromeDebuggerBridgeError::extension(Some("RESULT_TOO_LARGE"), "pdf is 60 MiB");
    assert_eq!(error.code(), RESULT_TOO_LARGE);
    assert_eq!(error.detail(), "pdf is 60 MiB");
}
