//! #29: `initialize` happens once per session. A repeated `initialize` on a
//! live session must be refused with JSON-RPC -32600 (same answer the HTTP
//! session middleware gives, `http/session_guards.rs::already_initialized_response`)
//! and must never answer with a different (latest) protocol version.
//!
//! The HTTP guard lives in the session middleware only; stdio (`main.rs::run_stdio`)
//! dispatches a repeat straight to rmcp's default `ServerHandler::initialize`,
//! which returns `get_info()` with the LATEST protocol. These tests drive both
//! real transports.

#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use support::HttpDaemon;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;

const OLD_PROTOCOL: &str = "2025-03-26";
const LATEST_PROTOCOL: &str = "2025-11-25";
/// Mirrors `support::BEARER` (private there).
const BEARER: &str = "synapse-tests-http-bearer";

fn init_params(protocol: &str, client: &str) -> Value {
    json!({
        "protocolVersion": protocol,
        "capabilities": {},
        "clientInfo": {"name": client, "version": "1"}
    })
}

async fn stdio_initialized_at(protocol: &str) -> anyhow::Result<StdioMcpClient> {
    let mut client = StdioMcpClient::launch(None)?;
    let first = client
        .request("initialize", init_params(protocol, "g1b-29-first"))
        .await?;
    assert_eq!(
        first["protocolVersion"], protocol,
        "first negotiation: {first}"
    );
    client
        .notify("notifications/initialized", json!({}))
        .await?;
    Ok(client)
}

#[tokio::test]
async fn must_fail_stdio_repeat_initialize_is_refused_with_invalid_request() -> anyhow::Result<()> {
    let mut client = stdio_initialized_at(OLD_PROTOCOL).await?;
    for (protocol, name) in [
        (OLD_PROTOCOL, "g1b-29-same"),
        (LATEST_PROTOCOL, "g1b-29-other"),
    ] {
        let error = client
            .request_error("initialize", init_params(protocol, name))
            .await?;
        assert_eq!(
            error["code"], -32600,
            "repeat initialize ({protocol}): {error}"
        );
    }
    // The session stays usable after the refusals.
    let tools = client.tools_list().await?;
    assert!(
        tools["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    );
    assert!(client.shutdown().await?.success());
    Ok(())
}

#[tokio::test]
async fn must_pass_stdio_first_initialize_negotiates_each_sessions_own_version()
-> anyhow::Result<()> {
    for protocol in [OLD_PROTOCOL, LATEST_PROTOCOL] {
        let mut client = stdio_initialized_at(protocol).await?;
        // tools_call fails on a JSON-RPC error or isError=true.
        client.tools_call("health", json!({})).await?;
        assert!(client.shutdown().await?.success());
    }
    Ok(())
}

async fn http_post(
    daemon: &HttpDaemon,
    session: &str,
    body: &Value,
) -> anyhow::Result<(u16, Value)> {
    let response = reqwest::Client::new()
        .post(daemon.url("/mcp"))
        .bearer_auth(BEARER)
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-session-id", session)
        .json(body)
        .send()
        .await?;
    let status = response.status().as_u16();
    Ok((status, support::decode_mcp_response(response).await?))
}

#[tokio::test]
async fn must_fail_http_repeat_initialize_on_live_session_is_refused() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g1b-29-http", &[]).await?;
    let id = session.session_id().to_owned();
    for (rpc_id, protocol) in [(71, OLD_PROTOCOL), (72, LATEST_PROTOCOL)] {
        let request = json!({"jsonrpc": "2.0", "id": rpc_id, "method": "initialize",
            "params": init_params(protocol, "g1b-29-repeat")});
        let (status, body) = http_post(&daemon, &id, &request).await?;
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["id"], rpc_id, "{body}");
        assert_eq!(body["error"]["code"], -32600, "{body}");
        assert_eq!(
            body["error"]["data"]["reason"],
            "session_already_initialized"
        );
    }
    // Same session keeps serving tools after the refusals.
    session.tools_call("health", json!({})).await?;
    drop(daemon.stop().await?);
    Ok(())
}
