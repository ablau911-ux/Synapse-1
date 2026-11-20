//! FU1 #28: media types are case-insensitive; body errors remain HTTP 400.
#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use support::HttpDaemon;

async fn post(daemon: &HttpDaemon, session: &str, body: &str) -> anyhow::Result<(u16, Value)> {
    let response = reqwest::Client::new()
        .post(daemon.url("/mcp"))
        .bearer_auth("synapse-tests-http-bearer")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .header("mcp-session-id", session)
        .header("content-type", "APPLICATION/JSON")
        .body(body.to_owned())
        .send()
        .await?;
    let status = response.status().as_u16();
    let value = support::decode_mcp_response(response).await?;
    Ok((status, value))
}

#[tokio::test]
async fn must_fail_uppercase_json_bad_body_returns_400_28() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let session = daemon.session("fu1-uppercase-invalid", &[]).await?;
    assert!(!session.session_id().is_empty());
    for (body, code) in [
        ("{bad", -32700),
        ("", -32700),
        ("[]", -32600),
        ("null", -32600),
    ] {
        let (status, result) = post(&daemon, session.session_id(), body).await?;
        assert_eq!(status, 400, "{body:?}: {result}");
        assert_eq!(result["error"]["code"], code);
        assert_eq!(result["id"], Value::Null);
    }
    // Separate request proves malformed input did not terminate the session.
    let ping = json!({"jsonrpc":"2.0", "id":2801, "method":"ping"});
    let (status, result) = post(&daemon, session.session_id(), &ping.to_string()).await?;
    assert_eq!(status, 200);
    assert_eq!(result, json!({"jsonrpc":"2.0", "id":2801, "result":{}}));
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn must_pass_uppercase_json_valid_ping_dispatches_28() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let session = daemon.session("fu1-uppercase-valid", &[]).await?;
    let body = json!({"jsonrpc":"2.0", "id":2802, "method":"ping"});
    let (status, result) = post(&daemon, session.session_id(), &body.to_string()).await?;
    assert_eq!(status, 200);
    assert_eq!(result, json!({"jsonrpc":"2.0", "id":2802, "result":{}}));
    daemon.stop().await?;
    Ok(())
}
