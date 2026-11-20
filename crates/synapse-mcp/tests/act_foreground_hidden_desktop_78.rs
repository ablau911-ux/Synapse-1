//! #78: a session that owns unrelated hidden-desktop resources (an agent-seat
//! launch on `agent:session`) must still be admitted to the visible-desktop
//! foreground lease and the `act operation=foreground` lane; the old refusal
//! `FOREGROUND_ACTIVATION_REFUSED` / `hidden_desktop_foreground_tier_refused`
//! must not come back.
//!
//! Contract: issue #78 "Expected" ("Hidden desktops owned by unrelated
//! resources in the same MCP session must not disable an explicitly
//! authorized visible-desktop action") + 2026-10-14 VBV PASS; current
//! admission: server/m2_tools.rs `acquire_tool_foreground_input_lease_with_ttl`
//! and m2.rs `acquire_foreground_input_lease_with_ttl` (profile + lease only).
//!
//! Fixture: `process operation=launch` of PING.EXE (self-terminating, ~20 s)
//! on the session's hidden desktop; precondition readback = the launch
//! response names the `SynapseAgent_` desktop. No real activation happens:
//! the foreground lane's focus step refuses the UNclaimed target by claim.

#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use g4_support::{
    effective_profile, full_capability_session, lease_status, set_window_target, synthetic_daemon,
};
use g4_window::{OwnedWindow, Placement};
use serde_json::json;
use support::HttpMcpSession;
use synapse_core::error_codes;

async fn launch_on_hidden_desktop(session: &mut HttpMcpSession) -> anyhow::Result<()> {
    let response = session
        .tools_call(
            "process",
            json!({
                "operation": "launch",
                "target": r"C:\Windows\System32\PING.EXE",
                "args": ["-n", "20", "127.0.0.1"],
                "desktop": "agent:session",
                "timeout_ms": 10_000
            }),
        )
        .await?;
    assert!(
        response["structuredContent"]
            .to_string()
            .contains("SynapseAgent_"),
        "precondition: the session owns a hidden-desktop resource: {response}"
    );
    Ok(())
}

#[tokio::test]
async fn must_pass_hidden_desktop_owner_acquires_the_foreground_lease() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = full_capability_session(&daemon).await?;
    launch_on_hidden_desktop(&mut session).await?;

    let result = session
        .tools_call(
            "act",
            json!({"operation": "lease_acquire", "ttl_ms": 5_000}),
        )
        .await?;

    let lease = &result["structuredContent"]["lease"];
    assert_eq!(lease["outcome"], "acquired", "{lease}");
    assert_eq!(lease["is_owner"], true, "{lease}");
    session
        .tools_call("act", json!({"operation": "lease_release"}))
        .await?;
    assert_eq!(lease_status(&mut session).await?["held"], false);
    drop(daemon.stop().await?);
    Ok(())
}

/// The foreground lane's second lease admission (taken before the focus step)
/// is not refused for hidden resources: the call gets as far as the focus
/// step's claim check, and the escalation unwinds.
#[tokio::test]
async fn must_fail_foreground_lane_refusal_is_the_claim_check_not_hidden_desktop()
-> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = full_capability_session(&daemon).await?;
    launch_on_hidden_desktop(&mut session).await?;
    let window = OwnedWindow::open(Placement {
        x: 1200,
        y: 660,
        w: 200,
        h: 120,
        topmost: false,
    });
    set_window_target(&mut session, window.hwnd()).await?;

    let error = session
        .tools_call_error(
            "act",
            json!({"operation": "foreground", "reason": "g4 #78 hidden-desktop admission",
                   "ttl_ms": 5_000,
                   "action": {"verb": "click", "x": 20, "y": 20, "coordinate_space": "window"}}),
        )
        .await?;

    assert_eq!(
        error["data"]["code"],
        error_codes::TARGET_CLAIM_NOT_FOUND,
        "{error}"
    );
    assert!(
        !error
            .to_string()
            .contains("hidden_desktop_foreground_tier_refused"),
        "hidden resources must not refuse the visible lane: {error}"
    );
    assert_eq!(window.click_count(), 0);
    assert_eq!(
        lease_status(&mut session).await?["held"],
        false,
        "lease released"
    );
    assert_eq!(
        effective_profile(&mut session).await?,
        "full_capability",
        "profile restored"
    );
    drop(daemon.stop().await?);
    Ok(())
}
