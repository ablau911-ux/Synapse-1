//! #299: several agents inside ONE MCP session, each with its own target,
//! driven through the real Streamable HTTP daemon and its public `target` and
//! `browser_tabs` facades with the strict client checks of `support`.
//!
//! Fixture: an isolated daemon (own temp home, DB and port; no Chrome bridge).
//! The two agents bind passive WINDOW targets on two windows this test opens
//! itself (`g4_window`): the owner's windows are never a fixture. Binding only
//! records the target; nothing is focused, typed into or clicked.
#![cfg(windows)]

mod g4_window;
#[allow(dead_code)]
mod support;

use anyhow::{Context, ensure};
use g4_window::{OwnedWindow, Placement};
use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};
use synapse_core::error_codes;

const AGENT_A: &str = "walk-a";
const AGENT_B: &str = "walk-b";

/// Opens two test-owned windows and proves the daemon lists both as visible,
/// non-browser, non-foreground windows (what a window target binds to).
async fn two_windows(session: &mut HttpMcpSession) -> anyhow::Result<(OwnedWindow, OwnedWindow)> {
    let open = |x| {
        OwnedWindow::open(Placement {
            x,
            y: 120,
            w: 200,
            h: 120,
            topmost: false,
        })
    };
    let (first, second) = (open(120), open(360));
    let listed = session
        .tools_call(
            "target",
            json!({"operation": "list", "exclude_minimized": true}),
        )
        .await?;
    let hwnds = listed["structuredContent"]["windows"]["windows"]
        .as_array()
        .context("target list returned no windows array")?
        .iter()
        .filter(|window| window["is_chromium"] == false && window["is_foreground"] == false)
        .filter_map(|window| window["hwnd"].as_i64())
        .collect::<Vec<_>>();
    ensure!(
        hwnds.contains(&first.hwnd()) && hwnds.contains(&second.hwnd()),
        "fixture precondition: the daemon must list both test-owned windows {:#x} and {:#x} as visible non-browser windows: {listed}",
        first.hwnd(),
        second.hwnd()
    );
    Ok((first, second))
}

async fn bind(session: &mut HttpMcpSession, agent: &str, hwnd: i64) -> anyhow::Result<Value> {
    session
        .tools_call(
            "target",
            json!({
                "operation": "set",
                "agent_id": agent,
                "target": {"kind": "window", "window_hwnd": hwnd}
            }),
        )
        .await
}

async fn bound_hwnd(session: &mut HttpMcpSession, agent: &str) -> anyhow::Result<Option<i64>> {
    let got = session
        .tools_call("target", json!({"operation": "get", "agent_id": agent}))
        .await?;
    Ok(got["structuredContent"]["target_state"]["current"]["window_hwnd"].as_i64())
}

fn error_code(error: &Value) -> &str {
    error["data"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn must_fail_two_agents_of_one_session_keep_their_own_targets() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("claude-code-299", &[]).await?;

    let listed = session.tools_list().await?;
    let schema_of = |name: &str| {
        listed["tools"]
            .as_array()
            .and_then(|tools| tools.iter().find(|tool| tool["name"] == name))
            .map(|tool| tool["inputSchema"].clone())
            .unwrap_or(Value::Null)
    };
    for name in ["act", "target", "browser_tabs", "browser_nav", "observe"] {
        ensure!(
            schema_of(name)["properties"]["agent_id"]["type"] == "string",
            "{name} must offer agent_id: {}",
            schema_of(name)
        );
    }
    ensure!(
        schema_of("health")["properties"]["agent_id"].is_null(),
        "health has no target and no agent_id"
    );

    let (own_a, own_b) = two_windows(&mut session).await?;
    let (window_a, window_b) = (own_a.hwnd(), own_b.hwnd());
    bind(&mut session, AGENT_A, window_a).await?;
    bind(&mut session, AGENT_B, window_b).await?;
    assert_eq!(bound_hwnd(&mut session, AGENT_A).await?, Some(window_a));
    assert_eq!(
        bound_hwnd(&mut session, AGENT_B).await?,
        Some(window_b),
        "walk-b's bind must not replace walk-a's target, nor walk-a's walk-b's"
    );

    // A call that names no agent has no target while agents hold targets.
    let forgot = session
        .tools_call_error("target", json!({"operation": "get"}))
        .await?;
    assert_eq!(
        error_code(&forgot),
        error_codes::TOOL_PARAMS_INVALID,
        "{forgot}"
    );
    assert!(
        forgot["message"]
            .as_str()
            .unwrap_or_default()
            .contains(AGENT_A),
        "{forgot}"
    );

    // A browser tool of walk-b resolves walk-b's own window, not walk-a's.
    let tabs = session
        .tools_call_error(
            "browser_tabs",
            json!({"operation": "list", "agent_id": AGENT_B}),
        )
        .await?;
    let message = tabs["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&format!("{window_b:#x}")) && !message.contains(&format!("{window_a:#x}")),
        "browser_tabs for walk-b must resolve walk-b's window {window_b:#x}: {tabs}"
    );

    // An explicit native-window override cannot read the other agent's target.
    let cross_read = session
        .tools_call_error(
            "observe",
            json!({"agent_id": AGENT_B, "window_hwnd": window_a}),
        )
        .await?;
    assert_eq!(error_code(&cross_read), error_codes::ACTION_TARGET_INVALID);

    // A named agent with no binding cannot fall back to the OS foreground.
    let unbound = session
        .tools_call_error("observe", json!({"agent_id": "walk-unbound"}))
        .await?;
    assert_eq!(error_code(&unbound), error_codes::TARGET_NOT_SET);

    let capture = session
        .tools_call_error(
            "livecapture",
            json!({"operation": "start", "window_hwnd": window_a}),
        )
        .await?;
    assert_eq!(error_code(&capture), error_codes::TOOL_PARAMS_INVALID);

    // A tool outside the 15 does not list agent_id: a strict client refuses
    // to send it, and the daemon refuses it when a client sends it anyway.
    let strict = session
        .tools_call_error("health", json!({"agent_id": AGENT_B}))
        .await
        .expect_err("a strict client must not send agent_id to health");
    let strict = format!("{strict:#}");
    assert!(
        strict.contains("CLIENT_SCHEMA_INVALID") && strict.contains("agent_id"),
        "{strict}"
    );
    let unsupported = session
        .tools_call_error_unchecked("health", json!({"agent_id": AGENT_B}))
        .await?;
    assert_eq!(
        error_code(&unsupported),
        error_codes::TOOL_PARAMS_INVALID,
        "{unsupported}"
    );

    // Two agents of one session never share a window.
    let shared = session
        .tools_call_error(
            "target",
            json!({
                "operation": "set",
                "agent_id": AGENT_B,
                "target": {"kind": "window", "window_hwnd": window_a}
            }),
        )
        .await?;
    assert_eq!(
        error_code(&shared),
        error_codes::ACTION_TARGET_INVALID,
        "{shared}"
    );
    assert_eq!(bound_hwnd(&mut session, AGENT_B).await?, Some(window_b));

    // When every agent cleared its target, the session works as before #299.
    for agent in [AGENT_A, AGENT_B] {
        session
            .tools_call("target", json!({"operation": "clear", "agent_id": agent}))
            .await?;
    }
    let plain = session
        .tools_call("target", json!({"operation": "get"}))
        .await?;
    assert!(
        plain["structuredContent"]["target_state"]["current"].is_null(),
        "{plain}"
    );
    Ok(())
}
