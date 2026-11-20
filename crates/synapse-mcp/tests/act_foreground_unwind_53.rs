//! #53: `act operation=foreground` acquires the foreground lease and escalates
//! the session to break_glass; EVERY exit (nested failure, failed action step,
//! success) must restore the prior profile and release a lease this call
//! acquired, never a lease it did not acquire; a bad request or a peer's lease
//! must refuse before any lease/profile change.
//!
//! Contract: issue #53 "Fix" + 2026-10-12 comment (fail-safe unwind on every
//! exit; `released_lease` only for a lease this call acquired; pre-existing
//! lease kept); server/background_router.rs `act_foreground_escalated_with`,
//! `validate_act_foreground_entry` (blank reason -> TOOL_PARAMS_INVALID before
//! side effects).
//!
//! Real path: HTTP daemon (synthetic foreground) -> `act` facade. Separate
//! readbacks after each call: `act operation=lease_status` and
//! `profile operation=status`. No action here reaches real input: verbs are
//! `read` (passive) or a `click` refused by the focus step's claim check.

#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use g4_support::{
    effective_profile, lease_status, normal_session, set_window_target, synthetic_daemon,
};
use g4_window::{OwnedWindow, Placement};
use serde_json::{Value, json};
use support::HttpMcpSession;
use synapse_core::error_codes;

const REASON: &str = "g4 #53 escalation unwind regression";

fn foreground_call(action: Value) -> Value {
    json!({"operation": "foreground", "reason": REASON, "ttl_ms": 5_000, "action": action})
}

fn assert_unwound(escalation: &Value, acquired: bool) {
    assert_eq!(escalation["acquired_lease"], acquired, "{escalation}");
    assert_eq!(escalation["released_lease"], acquired, "{escalation}");
    assert_eq!(escalation["prior_profile"], "normal_agent", "{escalation}");
    assert_eq!(escalation["escalated_to"], "break_glass", "{escalation}");
    assert_eq!(
        escalation["restored_profile"], "normal_agent",
        "{escalation}"
    );
    assert_eq!(escalation["profile_restored"], true, "{escalation}");
}

async fn assert_back_to_baseline(session: &mut HttpMcpSession) -> anyhow::Result<()> {
    assert_eq!(
        lease_status(session).await?["held"],
        false,
        "lease released"
    );
    assert_eq!(
        effective_profile(session).await?,
        "normal_agent",
        "profile restored"
    );
    Ok(())
}

fn target_window() -> OwnedWindow {
    OwnedWindow::open(Placement {
        x: 960,
        y: 60,
        w: 200,
        h: 120,
        topmost: false,
    })
}

#[tokio::test]
async fn must_pass_successful_step_unwinds_lease_and_profile() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-53-success").await?;
    let window = target_window();
    set_window_target(&mut session, window.hwnd()).await?;
    assert_back_to_baseline(&mut session).await?;

    let result = session
        .tools_call("act", foreground_call(json!({"verb": "read"})))
        .await?;

    let structured = &result["structuredContent"];
    assert_eq!(structured["action"]["ok"], true, "{structured}");
    assert_unwound(&structured["foreground"], true);
    assert_back_to_baseline(&mut session).await?;
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_nested_action_failure_still_unwinds() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-53-nested-failure").await?;

    // No session target: verb=read is refused (TARGET_NOT_SET) inside the step.
    let result = session
        .tools_call_expect_tool_error("act", foreground_call(json!({"verb": "read"})))
        .await?;

    let structured = &result["structuredContent"];
    assert_eq!(structured["action"]["ok"], false, "{structured}");
    assert_eq!(
        structured["action"]["result"]["error"]["code"],
        error_codes::TARGET_NOT_SET,
        "{structured}"
    );
    assert_unwound(&structured["foreground"], true);
    assert_back_to_baseline(&mut session).await?;
    drop(daemon.stop().await?);
    Ok(())
}

/// The action step itself returns Err (the focus step's claim check refuses an
/// unclaimed target): the unwind still runs before the error is returned.
#[tokio::test]
async fn must_fail_failed_action_step_still_unwinds() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-53-step-error").await?;
    let window = target_window();
    set_window_target(&mut session, window.hwnd()).await?;

    let error = session
        .tools_call_error(
            "act",
            foreground_call(
                json!({"verb": "click", "x": 20, "y": 20, "coordinate_space": "window"}),
            ),
        )
        .await?;

    assert_eq!(
        error["data"]["code"],
        error_codes::TARGET_CLAIM_NOT_FOUND,
        "{error}"
    );
    assert_eq!(window.click_count(), 0, "nothing delivered");
    assert_back_to_baseline(&mut session).await?;
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_a_lease_held_before_the_call_is_not_released() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-53-pre-held").await?;
    let window = target_window();
    set_window_target(&mut session, window.hwnd()).await?;
    session
        .tools_call(
            "act",
            json!({"operation": "lease_acquire", "ttl_ms": 30_000}),
        )
        .await?;
    assert_eq!(
        lease_status(&mut session).await?["is_owner"],
        true,
        "precondition"
    );

    let result = session
        .tools_call("act", foreground_call(json!({"verb": "read"})))
        .await?;

    assert_unwound(&result["structuredContent"]["foreground"], false);
    let lease = lease_status(&mut session).await?;
    assert_eq!(lease["held"], true, "{lease}");
    assert_eq!(lease["is_owner"], true, "pre-existing lease kept: {lease}");
    assert_eq!(effective_profile(&mut session).await?, "normal_agent");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_blank_reason_is_refused_before_any_state_change() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-53-blank-reason").await?;

    let error = session
        .tools_call_error(
            "act",
            json!({"operation": "foreground", "reason": "   ", "action": {"verb": "read"}}),
        )
        .await?;

    assert_eq!(
        error["data"]["code"],
        error_codes::TOOL_PARAMS_INVALID,
        "{error}"
    );
    assert_back_to_baseline(&mut session).await?;
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_peer_held_lease_refuses_escalation_and_keeps_the_peer_lease()
-> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut peer = normal_session(&daemon, "g4-53-peer").await?;
    let mut session = normal_session(&daemon, "g4-53-blocked").await?;
    peer.tools_call(
        "act",
        json!({"operation": "lease_acquire", "ttl_ms": 30_000}),
    )
    .await?;

    let error = session
        .tools_call_error("act", foreground_call(json!({"verb": "read"})))
        .await?;

    assert_eq!(
        error["data"]["code"],
        error_codes::ACTION_FOREGROUND_LEASE_BUSY,
        "{error}"
    );
    assert_eq!(
        lease_status(&mut peer).await?["is_owner"],
        true,
        "peer keeps its lease"
    );
    assert_eq!(
        effective_profile(&mut session).await?,
        "normal_agent",
        "never escalated"
    );
    drop(daemon.stop().await?);
    Ok(())
}
