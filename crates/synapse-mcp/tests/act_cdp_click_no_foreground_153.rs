//! #153: a click on a CDP web-node element id is delivered over CDP and must
//! not be refused with `A11Y_NO_FOREGROUND` when the OS has no foreground
//! window; native/UIA element clicks keep the fail-closed gate.
//!
//! Contract: issue #153 (act click by element id failed before delivery with
//! A11Y_NO_FOREGROUND); server/action_preflight.rs
//! `action_requires_live_foreground` doc; server/context.rs
//! `ensure_supported_use_allows_action_for`; server/m2_tools.rs `act_click`
//! passes `click_auto_wait_element_id` to the gate.
//!
//! #2406/#2407 (owner safety, `ElementScope`): a click by element id is acted
//! on only inside the session's target. So both cases set and claim a target
//! first. The native case keeps its meaning unchanged: a native element of the
//! session's own target window still meets the fail-closed foreground gate.
//! The CDP case cannot reach CDP dispatch here any more: the test-owned window
//! has no raw CDP endpoint, so its page cannot be told and `ElementScope`
//! refuses the web id before `act_click` (rows E7 and "an id without a page is
//! refused"; with no target the route answers TARGET_NOT_SET, rows RT1/E2).
//! That the foreground gate lets a CDP click through is still pinned by the
//! pure gate test `src/server/tests/cdp_no_foreground_153.rs`.
//!
//! Real path: HTTP daemon with `SYNAPSE_MCP_FORCE_NO_FOREGROUND=1` -> `act`
//! invoke click by element_id -> `target_act` -> (`ElementScope`) -> `act_click`.

#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use std::time::Duration;

use g4_support::{
    act_invoke_refused, claim_window, nested_code, no_foreground_daemon, normal_session,
    set_window_target,
};
use g4_window::{OwnedWindow, Placement};
use serde_json::json;
use synapse_core::error_codes;

fn element_window(y: i32) -> OwnedWindow {
    OwnedWindow::open(Placement {
        x: 1440,
        y,
        w: 200,
        h: 120,
        topmost: false,
    })
}

/// A DevTools page id (32 hex like Chromium's) that no endpoint of the
/// test-owned window lists.
const PAGE: &str = "A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1";

#[tokio::test]
async fn must_fail_cdp_element_click_without_a_known_target_page_is_refused_before_dispatch()
-> anyhow::Result<()> {
    let daemon = no_foreground_daemon().await?;
    let mut session = normal_session(&daemon, "g4-153-cdp").await?;
    let window = element_window(60);
    let element_id = synapse_a11y::cdp_element_id_for_target(window.hwnd(), PAGE, 42).to_string();
    let click = json!({
        "operation": "invoke",
        "action": {"verb": "click", "element_id": element_id}
    });

    // No target: refused before any lane; the text says what to do.
    let error = session.tools_call_error("act", click.clone()).await?;
    assert_eq!(
        error["data"]["code"],
        error_codes::TARGET_NOT_SET,
        "{error}"
    );
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("target operation=set") && message.contains("target operation=claim"),
        "the refusal tells the caller to set and claim a target: {error}"
    );

    // Target = this window, claimed: the window lists no page, so the page of
    // the web id cannot be checked and the click is refused before act_click.
    set_window_target(&mut session, window.hwnd()).await?;
    claim_window(&mut session, window.hwnd()).await?;
    let error = session.tools_call_error("act", click).await?;
    assert_eq!(
        error["data"]["code"],
        error_codes::ACTION_TARGET_INVALID,
        "{error}"
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|text| text.contains("cannot be told")),
        "{error}"
    );
    assert!(!window.wait_until(Duration::from_millis(500), |w| w.click_count() > 0));
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: a native UIA element click still needs the foreground.
#[tokio::test]
async fn must_fail_native_element_click_with_no_foreground_stays_fail_closed() -> anyhow::Result<()>
{
    let daemon = no_foreground_daemon().await?;
    let mut session = normal_session(&daemon, "g4-153-native").await?;
    let window = element_window(260);
    // #2406: the element is inside the session's own (claimed) target window.
    set_window_target(&mut session, window.hwnd()).await?;
    claim_window(&mut session, window.hwnd()).await?;
    let element_id = format!("0x{:x}:0000002a00000001", window.hwnd());

    let structured = act_invoke_refused(
        &mut session,
        json!({"verb": "click", "element_id": element_id}),
    )
    .await?;

    assert_eq!(
        structured["action"]["delegated_tool"], "act_click",
        "{structured}"
    );
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND),
        "{structured}"
    );
    assert_eq!(window.click_count(), 0);
    drop(daemon.stop().await?);
    Ok(())
}
