//! #7: a session-claimed, background-capable point click must not be refused
//! with `A11Y_NO_FOREGROUND` just because the OS has no foreground window,
//! while unclaimed point clicks stay fail-closed.
//!
//! Contract: issue #7 "Expected" + fix comment ("an exact session-targeted
//! point click now uses the background action scope gate and routes directly
//! to the target HWND PostMessage tier ... delivery_state=
//! delivered_and_pixel_verified, backend_tier=postmessage,
//! required_foreground=false"); server/context.rs
//! `ensure_supported_use_allows_action_scoped(background_proven)`;
//! server/m2_tools.rs `claimed_background_point_click`.
//!
//! Real path: HTTP daemon with `SYNAPSE_MCP_FORCE_NO_FOREGROUND=1` (the action
//! gate reads exactly `A11Y_NO_FOREGROUND`) -> `act` facade -> `target_act`
//! window-coordinate click -> `act_click` -> claimed PostMessage route. The
//! target is a test-owned, never-activated window whose WM_LBUTTONUP counter
//! and repaint are the delivery source of truth.

#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use std::time::Duration;

use g4_support::{
    act_invoke_ok, act_invoke_refused, claim_window, nested_code, no_foreground_daemon,
    normal_session, set_window_target,
};
use g4_window::{OwnedWindow, Placement};
use serde_json::{Value, json};
use synapse_core::error_codes;

const NO_DELIVERY_WINDOW: Duration = Duration::from_millis(750);

fn target_window(x: i32, y: i32) -> OwnedWindow {
    OwnedWindow::open(Placement {
        x,
        y,
        w: 240,
        h: 160,
        topmost: false,
    })
}

fn window_click(x: i32, y: i32) -> Value {
    json!({"verb": "click", "x": x, "y": y, "coordinate_space": "window", "clicks": 1})
}

fn assert_no_click_delivered(window: &OwnedWindow) {
    assert!(
        !window.wait_until(NO_DELIVERY_WINDOW, |w| w.click_count() > 0),
        "no click may reach window 0x{:x}; got {}",
        window.hwnd(),
        window.click_count()
    );
}

#[tokio::test]
async fn must_pass_claimed_background_point_click_is_delivered_with_no_foreground()
-> anyhow::Result<()> {
    let daemon = no_foreground_daemon().await?;
    let mut session = normal_session(&daemon, "g4-7-claimed").await?;
    let window = target_window(660, 60);
    set_window_target(&mut session, window.hwnd()).await?;
    claim_window(&mut session, window.hwnd()).await?;
    assert_eq!(window.click_count(), 0, "precondition: no click yet");

    let structured = act_invoke_ok(&mut session, window_click(120, 80)).await?;

    let action = &structured["action"];
    assert_eq!(action["ok"], true, "{structured}");
    assert_eq!(action["status"], "ok", "{structured}");
    assert_eq!(action["delegated_tool"], "act_click", "{structured}");
    assert_eq!(
        action["delivery_state"], "delivered_and_pixel_verified",
        "{structured}"
    );
    assert_eq!(
        action["result"]["backend_tier_used"], "postmessage",
        "{structured}"
    );
    assert_eq!(
        action["result"]["required_foreground"], false,
        "{structured}"
    );
    assert!(
        window.wait_until(Duration::from_secs(5), |w| w.click_count() >= 1),
        "the claimed window must receive the click"
    );
    assert_eq!(window.click_count(), 1, "exactly one click was requested");
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: without a claim the request is not background-proven, so
/// the gate keeps refusing with A11Y_NO_FOREGROUND and nothing is delivered.
#[tokio::test]
async fn must_fail_unclaimed_point_click_with_no_foreground_fails_closed() -> anyhow::Result<()> {
    let daemon = no_foreground_daemon().await?;
    let mut session = normal_session(&daemon, "g4-7-unclaimed").await?;
    let window = target_window(660, 260);
    set_window_target(&mut session, window.hwnd()).await?;

    let structured = act_invoke_refused(&mut session, window_click(120, 80)).await?;

    assert_eq!(structured["action"]["ok"], false, "{structured}");
    assert_eq!(
        structured["action"]["delegated_tool"], "act_click",
        "{structured}"
    );
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND),
        "{structured}"
    );
    assert_no_click_delivered(&window);
    drop(daemon.stop().await?);
    Ok(())
}

/// Boundary: a claimed target, but the window-space point lies outside it.
#[tokio::test]
async fn must_fail_claimed_click_outside_the_window_bounds_is_refused() -> anyhow::Result<()> {
    let daemon = no_foreground_daemon().await?;
    let mut session = normal_session(&daemon, "g4-7-out-of-bounds").await?;
    let window = target_window(660, 460);
    set_window_target(&mut session, window.hwnd()).await?;
    claim_window(&mut session, window.hwnd()).await?;

    let structured = act_invoke_refused(&mut session, window_click(5_000, 5_000)).await?;

    assert_eq!(structured["action"]["ok"], false, "{structured}");
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::ACTION_TARGET_INVALID),
        "{structured}"
    );
    assert_no_click_delivered(&window);
    drop(daemon.stop().await?);
    Ok(())
}
