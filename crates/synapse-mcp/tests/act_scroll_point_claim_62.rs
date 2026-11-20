//! #62 item 1: `act invoke verb=scroll x,y` delivers WM_MOUSEWHEEL to whatever
//! top-level window is under the point (m2/scroll.rs `execute_targeted_scroll_actions`
//! -> `windows_hwnd_message_scroll_readback` -> `WindowFromPoint`). The claim
//! check before it (`server/target_claims.rs` `ensure_target_claim_allows_session`)
//! only refuses a PEER's claim, so an UNCLAIMED window (the human's) is scrolled
//! and reported as success (first review FAILED on build b5c9e097).
//!
//! Contract (issue body: "require claim/window verification like act_click";
//! act_click's background point route, m2/click/point_post.rs
//! `point_click_route` + `unclaimed_point_target_error`): a session's
//! coordinate scroll is delivered only to a window this session has claimed;
//! otherwise it is refused before delivery with `TARGET_CLAIM_NOT_FOUND`.
//!
//! Real path: HTTP daemon -> `act` facade -> `target_act` verb=scroll ->
//! `act_scroll`. Fixture windows are created by the test, topmost and never
//! activated; their message counters are the delivery source of truth.

#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use std::time::Duration;

use g4_support::{
    act_invoke_ok, act_invoke_refused, claim_window, nested_code, normal_session, synthetic_daemon,
};
use g4_window::{OwnedWindow, Placement, root_at};
use serde_json::{Value, json};
use synapse_core::error_codes;

/// Absence of delivery is observed over this bounded window.
const NO_DELIVERY_WINDOW: Duration = Duration::from_millis(750);

fn topmost_window(x: i32, y: i32) -> OwnedWindow {
    OwnedWindow::open(Placement {
        x,
        y,
        w: 240,
        h: 160,
        topmost: true,
    })
}

/// Scroll down one notch (#219 units) at the window's centre, screen space.
fn scroll_at_center(window: &OwnedWindow) -> Value {
    let (x, y) = window.center();
    json!({"verb": "scroll", "args": ["0", "-1"], "x": x, "y": y})
}

/// Precondition readback: the daemon's hit test resolves the point to `window`.
fn assert_point_resolves_to(window: &OwnedWindow) {
    let (x, y) = window.center();
    assert_eq!(
        root_at(x, y),
        Some(window.hwnd()),
        "precondition: the scroll point ({x},{y}) is over test window 0x{:x}",
        window.hwnd()
    );
    assert_eq!(
        window.wheel_count(),
        0,
        "precondition: no wheel received yet"
    );
}

fn assert_no_wheel_delivered(window: &OwnedWindow) {
    assert!(
        !window.wait_until(NO_DELIVERY_WINDOW, |w| w.wheel_count() > 0),
        "no WM_MOUSEWHEEL may reach window 0x{:x}; got {}",
        window.hwnd(),
        window.wheel_count()
    );
}

fn assert_refused(structured: &Value, code: &str) {
    assert_eq!(structured["action"]["ok"], false, "{structured}");
    assert_eq!(
        structured["action"]["delegated_tool"], "act_scroll",
        "{structured}"
    );
    assert_eq!(
        nested_code(structured).as_deref(),
        Some(code),
        "{structured}"
    );
}

#[tokio::test]
async fn must_fail_scroll_at_point_over_an_unclaimed_window_is_refused_before_delivery()
-> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-62-unclaimed").await?;
    let window = topmost_window(60, 60);
    assert_point_resolves_to(&window);

    let structured = act_invoke_refused(&mut session, scroll_at_center(&window)).await?;

    assert_refused(&structured, error_codes::TARGET_CLAIM_NOT_FOUND);
    assert_no_wheel_delivered(&window);
    drop(daemon.stop().await?);
    Ok(())
}

/// The first-review scenario: the session holds a claim on ANOTHER window and
/// scrolls over an unclaimed one; holding some claim must not authorize it.
#[tokio::test]
async fn must_fail_scroll_over_unclaimed_window_while_claiming_another_is_refused()
-> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-62-other-claim").await?;
    let claimed = topmost_window(60, 260);
    let unclaimed = topmost_window(360, 260);
    claim_window(&mut session, claimed.hwnd()).await?;
    assert_point_resolves_to(&unclaimed);

    let structured = act_invoke_refused(&mut session, scroll_at_center(&unclaimed)).await?;

    assert_refused(&structured, error_codes::TARGET_CLAIM_NOT_FOUND);
    assert_no_wheel_delivered(&unclaimed);
    assert_eq!(
        claimed.wheel_count(),
        0,
        "the claimed window was not the target"
    );
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: a scroll over this session's own claimed window is
/// delivered to that window, exactly one notch.
#[tokio::test]
async fn must_pass_scroll_at_point_over_own_claimed_window_is_delivered() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-62-own-claim").await?;
    let window = topmost_window(60, 460);
    claim_window(&mut session, window.hwnd()).await?;
    assert_point_resolves_to(&window);

    let structured = act_invoke_ok(&mut session, scroll_at_center(&window)).await?;

    assert_eq!(structured["action"]["ok"], true, "{structured}");
    assert_eq!(
        structured["action"]["delegated_tool"], "act_scroll",
        "{structured}"
    );
    assert_eq!(
        structured["action"]["result"]["wheel_event_count"], 1,
        "{structured}"
    );
    assert!(
        window.wait_until(Duration::from_secs(5), |w| w.wheel_count() >= 1),
        "the claimed window must receive the wheel message"
    );
    assert_eq!(window.wheel_count(), 1, "exactly one notch was requested");
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: a peer session's claim still refuses with TARGET_CO_OWNED.
#[tokio::test]
async fn must_fail_scroll_over_a_peer_claimed_window_is_refused_as_co_owned() -> anyhow::Result<()>
{
    let daemon = synthetic_daemon().await?;
    let mut owner = normal_session(&daemon, "g4-62-owner").await?;
    let mut intruder = normal_session(&daemon, "g4-62-intruder").await?;
    let window = topmost_window(360, 460);
    claim_window(&mut owner, window.hwnd()).await?;
    assert_point_resolves_to(&window);

    let structured = act_invoke_refused(&mut intruder, scroll_at_center(&window)).await?;

    assert_refused(&structured, error_codes::TARGET_CO_OWNED);
    assert_no_wheel_delivered(&window);
    drop(daemon.stop().await?);
    Ok(())
}
