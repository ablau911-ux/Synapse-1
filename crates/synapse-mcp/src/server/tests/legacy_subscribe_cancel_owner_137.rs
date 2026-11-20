//! Direct legacy-handler coverage, including sessionless/debug invocation lanes.
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};

use crate::server::SynapseService;

fn seed(service: &SynapseService) -> String {
    let sse = service.sse_state().unwrap();
    let params = serde_json::from_value(json!({"kinds": []})).unwrap();
    let created =
        crate::m3::subscribe::subscribe_to_events(&sse, &params, Some("fu4-owner137".to_owned()))
            .unwrap();
    assert_eq!(
        sse.subscription_ids_for_session("fu4-owner137").unwrap(),
        vec![created.subscription_id.clone()]
    );
    assert_eq!(sse.active_subscription_count(), 1);
    created.subscription_id
}

#[tokio::test]
async fn must_pass_legacy_owner_cancels_own_subscription_137() {
    let root = tempfile::tempdir().unwrap();
    let service = crate::test_support::service_in(root.path()).unwrap();
    let id = seed(&service);
    let params = serde_json::from_value(json!({"subscription_id": id})).unwrap();
    let response = crate::http::with_current_mcp_session_id(
        Some("fu4-owner137".to_owned()),
        service.subscribe_cancel(Parameters(params)),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({"cancelled":true,"reason":"ok"})
    );
    let sse = service.sse_state().unwrap();
    assert_eq!(sse.active_subscription_count(), 0);
    assert_eq!(
        sse.subscription_ids_for_session("fu4-owner137").unwrap(),
        Vec::<String>::new()
    );
}

async fn refused_cancel(session: Option<String>, id_override: Option<&str>, code: &str) {
    let root = tempfile::tempdir().unwrap();
    let service = crate::test_support::service_in(root.path()).unwrap();
    let id = seed(&service);
    let params =
        serde_json::from_value(json!({"subscription_id":id_override.unwrap_or(&id)})).unwrap();
    let result = crate::http::with_current_mcp_session_id(
        session,
        service.subscribe_cancel(Parameters(params)),
    )
    .await;
    // Check state before demanding the error: unowned cancellation must not remove it.
    let sse = service.sse_state().unwrap();
    assert_eq!(
        sse.subscription_ids_for_session("fu4-owner137").unwrap(),
        vec![id.clone()]
    );
    assert_eq!(
        sse.active_subscription_count(),
        1,
        "subscription must remain live"
    );
    let error = result.err().unwrap();
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|v| v.get("code"))
            .and_then(Value::as_str),
        Some(code)
    );
    // Successful owner cancellation afterwards independently proves the handle survived.
    let params = serde_json::from_value(json!({"subscription_id":id})).unwrap();
    let response = crate::http::with_current_mcp_session_id(
        Some("fu4-owner137".to_owned()),
        service.subscribe_cancel(Parameters(params)),
    )
    .await
    .unwrap()
    .0;
    assert!(response.cancelled);
    assert_eq!(sse.active_subscription_count(), 0);
}

#[tokio::test]
async fn must_fail_legacy_foreign_session_preserves_live_subscription_137() {
    refused_cancel(
        Some("fu4-intruder137".to_owned()),
        None,
        "SUBSCRIPTION_NOT_FOUND",
    )
    .await;
}

#[tokio::test]
async fn must_fail_legacy_sessionless_preserves_live_subscription_137() {
    refused_cancel(None, None, "TOOL_PARAMS_INVALID").await;
}

#[tokio::test]
async fn must_fail_legacy_empty_id_without_state_change_137() {
    refused_cancel(
        Some("fu4-owner137".to_owned()),
        Some("  "),
        "TOOL_PARAMS_INVALID",
    )
    .await;
}

#[tokio::test]
async fn must_fail_legacy_unknown_id_without_state_change_137() {
    refused_cancel(
        Some("fu4-owner137".to_owned()),
        Some("unknown-fu4-137"),
        "SUBSCRIPTION_NOT_FOUND",
    )
    .await;
}
