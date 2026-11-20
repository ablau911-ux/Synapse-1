//! #162: in `act operation=foreground`, an input verb (click/type/press/scroll)
//! must first run the verified claimed-target focus step and only then deliver
//! through the foreground tier; the background preflight belongs to
//! `operation=invoke` only.
//!
//! Contract: issue #162 "Expected"; server/act_foreground_focus.rs module doc;
//! server/background_router.rs `act_foreground_action_step` ->
//! `act_foreground_focused_action` -> `act_foreground_focus_key_target`
//! (`target_act_focus_window_preflight` first: the session must own a live
//! claim on its target).
//!
//! Real route, no real activation: the session target is a test-owned window
//! that is NOT claimed, so the focus step itself refuses with its own
//! `verb=focus_window` claim message before any delivery. If the foreground
//! lane skipped the focus step, the click would instead reach `act_click`
//! (different message) — that is what these tests detect. The successful
//! focus-then-click on a real claimed target is a live VBV item (it takes the
//! human foreground).

#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use g4_support::{
    act_invoke_refused, nested_code, normal_session, set_window_target, synthetic_daemon,
};
use g4_window::{OwnedWindow, Placement};
use serde_json::{Value, json};
use synapse_core::error_codes;

const FOCUS_STEP_CLAIM_MESSAGE: &str =
    "target_act verb=focus_window requires this MCP session to own a live target_claim";

fn target_window(y: i32) -> OwnedWindow {
    OwnedWindow::open(Placement {
        x: 1200,
        y,
        w: 200,
        h: 120,
        topmost: false,
    })
}

fn click() -> Value {
    json!({"verb": "click", "x": 20, "y": 20, "coordinate_space": "window"})
}

#[tokio::test]
async fn must_fail_foreground_click_runs_the_focus_step_before_delivery() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-162-click").await?;
    let window = target_window(60);
    set_window_target(&mut session, window.hwnd()).await?;

    let error = session
        .tools_call_error(
            "act",
            json!({"operation": "foreground", "reason": "g4 #162 focus-first", "ttl_ms": 5_000,
                   "action": click()}),
        )
        .await?;

    assert_eq!(
        error["data"]["code"],
        error_codes::TARGET_CLAIM_NOT_FOUND,
        "{error}"
    );
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(FOCUS_STEP_CLAIM_MESSAGE),
        "the refusal must come from the focus step, not from a delivery path: {error}"
    );
    assert_eq!(window.click_count(), 0, "nothing delivered");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_foreground_scroll_also_runs_the_focus_step_first() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-162-scroll").await?;
    let window = target_window(260);
    set_window_target(&mut session, window.hwnd()).await?;

    let error = session
        .tools_call_error(
            "act",
            json!({"operation": "foreground", "reason": "g4 #162 focus-first", "ttl_ms": 5_000,
                   "action": {"verb": "scroll", "args": ["0", "-1"]}}),
        )
        .await?;

    assert_eq!(
        error["data"]["code"],
        error_codes::TARGET_CLAIM_NOT_FOUND,
        "{error}"
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains(FOCUS_STEP_CLAIM_MESSAGE),
        "{error}"
    );
    assert_eq!(window.wheel_count(), 0, "nothing delivered");
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: `operation=invoke` keeps the target-scoped route with no
/// focus step (the unclaimed background point click is refused by act_click's
/// own claim route, delegated_tool=act_click).
#[tokio::test]
async fn must_fail_invoke_click_keeps_the_target_scoped_route() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-162-invoke").await?;
    let window = target_window(460);
    set_window_target(&mut session, window.hwnd()).await?;

    let structured = act_invoke_refused(&mut session, click()).await?;

    assert_eq!(
        structured["action"]["delegated_tool"], "act_click",
        "{structured}"
    );
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::TARGET_CLAIM_NOT_FOUND),
        "{structured}"
    );
    assert!(
        !structured.to_string().contains(FOCUS_STEP_CLAIM_MESSAGE),
        "invoke must not run the foreground focus step: {structured}"
    );
    assert_eq!(window.click_count(), 0);
    drop(daemon.stop().await?);
    Ok(())
}
