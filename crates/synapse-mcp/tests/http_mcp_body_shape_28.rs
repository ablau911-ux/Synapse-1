//! #28 on the REAL daemon router (session middleware + rmcp): a POST /mcp body
//! that is valid JSON but not a JSON-RPC client message (`[]`, `null`, `42`,
//! a batch, an object without method/result) is a client payload error and
//! must answer 400 with JSON-RPC -32600, never rmcp's 415. Truncated/empty
//! JSON answers 400 -32700. 415 stays reserved for a wrong/missing media type.
//! Contract: issue #28 body + 2026-09-08 acceptance comment.

#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession, decode_mcp_response};

/// Mirrors `support::BEARER` (private there); the daemon is started with it.
const BEARER: &str = "synapse-tests-http-bearer";
const RUN_ID: &str = "g1b-28";

/// Raw authenticated POST /mcp; `content_type: None` omits the header.
async fn post_raw(
    daemon: &HttpDaemon,
    session: Option<&str>,
    content_type: Option<&str>,
    body: &str,
) -> anyhow::Result<(u16, String)> {
    let mut request = reqwest::Client::new()
        .post(daemon.url("/mcp"))
        .bearer_auth(BEARER)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .body(body.to_owned());
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await?;
    Ok((response.status().as_u16(), response.text().await?))
}

fn jsonrpc_error_code(body: &str) -> Option<i64> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .pointer("/error/code")?
        .as_i64()
}

async fn exists(session: &mut HttpMcpSession, key: &str) -> anyhow::Result<Value> {
    let result = session
        .tools_call(
            "workspace",
            json!({"operation": "exists", "exists": {"run_id": RUN_ID, "key": key}}),
        )
        .await?;
    Ok(result["structuredContent"]["exists"].clone())
}

fn put_call(id: u64, key: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
        "name": "workspace",
        "arguments": {"operation": "put", "put": {"run_id": RUN_ID, "key": key, "value": {"n": 1}}}
    }})
}

#[tokio::test]
async fn must_fail_session_bound_non_message_json_is_400_invalid_request() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let session = daemon.session("g1b-28-shape", &[]).await?;
    for body in [
        "[]",
        "null",
        "42",
        "\"text\"",
        "{\"foo\":1}",
        "{\"jsonrpc\":\"2.0\"}",
    ] {
        let (status, text) = post_raw(
            &daemon,
            Some(session.session_id()),
            Some("application/json"),
            body,
        )
        .await?;
        assert_eq!(status, 400, "body {body} must be 400, got {status}: {text}");
        assert_eq!(
            jsonrpc_error_code(&text),
            Some(-32600),
            "body {body}: {text}"
        );
        let parsed: Value = serde_json::from_str(&text)?;
        assert_eq!(parsed["id"], Value::Null, "body {body}: {text}");
    }
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_rejected_batch_is_400_and_never_dispatches_its_write() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g1b-28-batch", &[]).await?;
    let key = "batch-must-remain-absent";
    assert_eq!(
        exists(&mut session, key).await?["exists"],
        false,
        "precondition"
    );

    let batch = Value::Array(vec![put_call(90, key)]).to_string();
    let (status, text) = post_raw(
        &daemon,
        Some(session.session_id()),
        Some("application/json"),
        &batch,
    )
    .await?;
    assert_eq!(status, 400, "batch must be 400: {text}");
    assert_eq!(jsonrpc_error_code(&text), Some(-32600), "{text}");

    let after = exists(&mut session, key).await?;
    assert_eq!(
        after["exists"], false,
        "rejected batch wrote a row: {after}"
    );
    assert_eq!(after["physical_row_present"], false, "{after}");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_truncated_and_empty_json_on_live_session_is_400_parse_error()
-> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let session = daemon.session("g1b-28-parse", &[]).await?;
    for body in ["{\"jsonrpc\":", ""] {
        let (status, text) = post_raw(
            &daemon,
            Some(session.session_id()),
            Some("application/json"),
            body,
        )
        .await?;
        assert_eq!(status, 400, "body {body:?}: {text}");
        assert_eq!(
            jsonrpc_error_code(&text),
            Some(-32700),
            "body {body:?}: {text}"
        );
    }
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_wrong_or_missing_media_type_keeps_415() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let session = daemon.session("g1b-28-media", &[]).await?;
    let id = Some(session.session_id());
    assert_eq!(
        post_raw(&daemon, id, Some("text/plain"), "{}").await?.0,
        415
    );
    assert_eq!(post_raw(&daemon, id, None, "{}").await?.0, 415);
    assert_eq!(
        post_raw(&daemon, id, Some("text/plain"), "{bad").await?.0,
        415
    );
    drop(daemon.stop().await?);
    Ok(())
}

/// `application/jsonx` is not `application/json`: a malformed body under it is
/// a media-type problem (415), not a Synapse JSON parse error (400).
#[tokio::test]
async fn must_fail_json_lookalike_media_type_is_not_classified_as_json() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let session = daemon.session("g1b-28-jsonx", &[]).await?;
    let (status, text) = post_raw(
        &daemon,
        Some(session.session_id()),
        Some("application/jsonx"),
        "{bad",
    )
    .await?;
    assert_eq!(status, 415, "application/jsonx + malformed body: {text}");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_valid_messages_still_dispatch_and_write() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g1b-28-valid", &[]).await?;
    let id = Some(session.session_id().to_owned());
    let key = "valid-put-must-exist";

    let (status, text) = post_raw(
        &daemon,
        id.as_deref(),
        Some("application/json; charset=utf-8"),
        &put_call(91, key).to_string(),
    )
    .await?;
    assert_eq!(status, 200, "{text}");
    let after = exists(&mut session, key).await?;
    assert_eq!(after["exists"], true, "valid put must write: {after}");

    let ping = json!({"jsonrpc": "2.0", "id": 92, "method": "ping"}).to_string();
    let response = reqwest::Client::new()
        .post(daemon.url("/mcp"))
        .bearer_auth(BEARER)
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-session-id", session.session_id())
        .body(ping)
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(decode_mcp_response(response).await?["id"], 92);

    let cancelled = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": "g1b-28-never-started"}})
    .to_string();
    let (status, _) =
        post_raw(&daemon, id.as_deref(), Some("application/json"), &cancelled).await?;
    assert_eq!(status, 202, "valid notification stays accepted");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_sessionless_non_initialize_keeps_404_admission() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let list = json!({"jsonrpc": "2.0", "id": 93, "method": "tools/list"}).to_string();
    let (status, text) = post_raw(&daemon, None, Some("application/json"), &list).await?;
    assert_eq!(status, 404, "{text}");
    let body: Value = serde_json::from_str(&text)?;
    assert_eq!(body["code"], "HTTP_SESSION_INVALID");
    assert_eq!(body["reason"], "session_header_missing");
    drop(daemon.stop().await?);
    Ok(())
}
