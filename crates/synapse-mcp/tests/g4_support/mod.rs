//! Shared HTTP-daemon helpers for the G4 `act` regression tests.
//!
//! Every call goes through `support::HttpMcpSession` (schema-validating client
//! parity, D1). A delegated action that fails is returned by the daemon as an
//! `isError: true` result whose `structuredContent` still carries the nested
//! `action` (#10/#221), so refusals are read with
//! `HttpMcpSession::tools_call_expect_tool_error` (harness seam, see the G4
//! report: same contract as `StdioMcpClient::tools_call_expect_tool_error`).

#![allow(dead_code)]

use serde_json::{Value, json};

use crate::support::{HttpDaemon, HttpMcpSession};

/// Local-agent capability shared with tests/shell_profile_policy.rs: a client
/// named `synapse-local-model-agent` with this header gets `full_capability`.
pub const CAPABILITY: &str = "g4-isolated-local-agent-capability-0123456789";

/// Daemon whose action gate sees the synthetic Notepad foreground
/// (`SYNAPSE_MCP_SYNTHETIC_FIXTURE=notepad`), never the host's real one.
pub async fn synthetic_daemon() -> anyhow::Result<HttpDaemon> {
    HttpDaemon::start(&[
        ("SYNAPSE_LOCAL_AGENT_CAPABILITY", CAPABILITY),
        ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
    ])
    .await
}

/// Daemon whose action gate reads `A11Y_NO_FOREGROUND`
/// (`SYNAPSE_MCP_FORCE_NO_FOREGROUND=1`, same signal as a null
/// `GetForegroundWindow`).
pub async fn no_foreground_daemon() -> anyhow::Result<HttpDaemon> {
    HttpDaemon::start(&[
        ("SYNAPSE_LOCAL_AGENT_CAPABILITY", CAPABILITY),
        ("SYNAPSE_MCP_FORCE_NO_FOREGROUND", "1"),
    ])
    .await
}

/// Daemon with no foreground fixture (real OS foreground is only READ).
pub async fn plain_daemon() -> anyhow::Result<HttpDaemon> {
    HttpDaemon::start(&[("SYNAPSE_LOCAL_AGENT_CAPABILITY", CAPABILITY)]).await
}

pub async fn full_capability_session(daemon: &HttpDaemon) -> anyhow::Result<HttpMcpSession> {
    let mut session = daemon
        .session(
            "synapse-local-model-agent",
            &[("X-Synapse-Local-Agent-Capability", CAPABILITY)],
        )
        .await?;
    assert_eq!(effective_profile(&mut session).await?, "full_capability");
    Ok(session)
}

pub async fn normal_session(daemon: &HttpDaemon, name: &str) -> anyhow::Result<HttpMcpSession> {
    let mut session = daemon.session(name, &[]).await?;
    assert_eq!(effective_profile(&mut session).await?, "normal_agent");
    Ok(session)
}

pub async fn effective_profile(session: &mut HttpMcpSession) -> anyhow::Result<Value> {
    let status = session
        .tools_call("profile", json!({"operation": "status"}))
        .await?;
    Ok(status["structuredContent"]["status"]["snapshot"]["profile"].clone())
}

/// `target set` + read back that the session target is exactly `hwnd`.
pub async fn set_window_target(session: &mut HttpMcpSession, hwnd: i64) -> anyhow::Result<()> {
    session
        .tools_call(
            "target",
            json!({"operation": "set", "target": {"kind": "window", "window_hwnd": hwnd}}),
        )
        .await?;
    let get = session
        .tools_call("target", json!({"operation": "get"}))
        .await?;
    let text = get["structuredContent"].to_string();
    assert!(
        text.contains(&hwnd.to_string()),
        "precondition: session target is HWND {hwnd}: {get}"
    );
    Ok(())
}

/// `target claim` on a window + read back the owner is this session.
pub async fn claim_window(session: &mut HttpMcpSession, hwnd: i64) -> anyhow::Result<()> {
    let response = session
        .tools_call(
            "target",
            json!({
                "operation": "claim",
                "target": {"kind": "window", "window_hwnd": hwnd},
                "ttl_ms": 120_000
            }),
        )
        .await?;
    let owner = &response["structuredContent"]["claim"]["claim"]["owner_session_id"];
    assert_eq!(
        owner.as_str(),
        Some(session.session_id()),
        "precondition: claim on HWND {hwnd} is owned by this session: {response}"
    );
    Ok(())
}

/// `act operation=invoke` that must succeed; returns `structuredContent`.
pub async fn act_invoke_ok(session: &mut HttpMcpSession, action: Value) -> anyhow::Result<Value> {
    let result = session
        .tools_call("act", json!({"operation": "invoke", "action": action}))
        .await?;
    Ok(result["structuredContent"].clone())
}

/// `act operation=invoke` whose nested action must fail (`isError: true`);
/// returns `structuredContent` (outer `ActResponse` with the nested action).
pub async fn act_invoke_refused(
    session: &mut HttpMcpSession,
    action: Value,
) -> anyhow::Result<Value> {
    let result = session
        .tools_call_expect_tool_error("act", json!({"operation": "invoke", "action": action}))
        .await?;
    Ok(result["structuredContent"].clone())
}

/// Nested delegated error code of an `act` response.
pub fn nested_code(structured: &Value) -> Option<String> {
    structured["action"]["result"]["error"]["code"]
        .as_str()
        .map(str::to_owned)
}

pub async fn lease_status(session: &mut HttpMcpSession) -> anyhow::Result<Value> {
    let result = session
        .tools_call("act", json!({"operation": "lease_status"}))
        .await?;
    Ok(result["structuredContent"]["lease"].clone())
}
