//! #79/#107 regression: the background-WebView2-click refusal must cover the
//! raw point click surface (`m2::click::point_post`, added by #107), not only
//! the pre-existing UIA-element surface. Before this fix,
//! `preflight_background_webview_uia_click` returned `Ok(())` immediately for
//! any `ActClickTarget::Point`, so a claimed-but-background point click could
//! be posted straight into a WebView2 child HWND with no CDP-availability
//! check -- the exact "PostMessage tier reported delivered, no observed delta"
//! failure mode from #79, reintroduced for the new point-click route.
//!
//! The `must_fail_*_preflight_*` tests drive the real preflight the handler
//! calls and fail if the fix is reverted. The builder/label tests below cover
//! the refusal message only: `webview2_background_click_refusal_error` and
//! `click_target_kind_label` take plain data. Driving the preflight all the way
//! to the WebView2 refusal needs an injectable CDP-diagnostics and foreground
//! probe (not yet a seam); full-state verification against a real Finance
//! Manager WebView2 window remains a manual FSV item per D1.
use serde_json::{Value, json};
use synapse_core::{CdpDiagnostics, CdpStatus, error_codes};

use super::*;
use crate::m2::ActClickTarget;

fn error_code(error: &ErrorData) -> Option<String> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

// `ActClickElementTarget`/`ActClickPointTarget` are private to `m2::click`; the
// public boundary for constructing an `ActClickTarget` outside that module is
// deserialization (`#[serde(untagged)]`), the same route used by every other
// cross-module caller (e.g. `background_router::target_act_click_params`).
fn point_target() -> ActClickTarget {
    serde_json::from_value(json!({ "x": 60, "y": 325 })).expect("valid synthetic point target")
}

fn element_target() -> ActClickTarget {
    serde_json::from_value(json!({ "element_id": "0x1000:1" }))
        .expect("valid synthetic element target")
}

fn point_params_bound_to(hwnd: i64) -> ActClickParams {
    serde_json::from_value(json!({
        "target": { "x": 60, "y": 325 },
        "verify_target_window_hwnd": hwnd,
    }))
    .expect("valid synthetic point click params")
}

/// Drives the real preflight the `act_click` handler calls
/// (`m2_tools.rs` `preflight_background_webview_uia_click(&params)`) with a
/// point target bound to a window. The #79/#107 fix makes the preflight
/// resolve the point target's root window instead of returning `Ok(())` for
/// every `ActClickTarget::Point`. HWND 0 is never a live window, so the
/// resolution must refuse with `ACTION_TARGET_INVALID`; a reverted fix (early
/// `Ok` for Point) returns `Ok(())` and fails this test.
#[cfg(windows)]
#[test]
fn must_fail_point_click_preflight_resolves_the_bound_window_instead_of_skipping() {
    let error = preflight_background_webview_uia_click(&point_params_bound_to(0))
        .expect_err("a point click bound to a dead HWND must not pass the WebView2 preflight");
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_TARGET_INVALID.to_owned())
    );
    let message = error.message.to_string();
    // #315 (edcdb26c): the refusal goes through the one window-check mapping,
    // which names the bound field and the point target.
    assert!(
        message.contains("verify_target_window_hwnd")
            && message.contains("point target")
            && message.contains("HWND was null"),
        "{message}"
    );
}

/// Same real preflight, element surface (the pre-#107 path) bound to a dead
/// HWND: it must keep refusing with `ACTION_TARGET_INVALID`, so the point fix
/// did not change the element branch.
#[cfg(windows)]
#[test]
fn must_fail_element_click_preflight_still_resolves_its_window() {
    let params: ActClickParams = serde_json::from_value(json!({
        "target": { "element_id": "0x0:1" },
    }))
    .expect("valid synthetic element click params");
    let error = preflight_background_webview_uia_click(&params)
        .expect_err("an element click on a dead HWND must not pass the WebView2 preflight");
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_TARGET_INVALID.to_owned())
    );
}

#[test]
fn must_pass_point_click_kind_label_names_the_raw_point_surface() {
    // #107's new surface must be named distinctly from the pre-existing
    // UIA-element surface in the refusal message, not silently absorbed into
    // generic "UIA Invoke" wording that would mislead a point-click caller.
    assert_eq!(click_target_kind_label(&point_target()), "point");
    assert_eq!(click_target_kind_label(&element_target()), "UIA element");
}

#[test]
fn must_fail_point_click_refused_before_delivery_when_webview2_cdp_unreachable() {
    let cdp = CdpDiagnostics::unreachable_with_probe(
        "FinanceManager.exe",
        error_codes::A11Y_CDP_UNREACHABLE,
        vec![9227],
        "WebView2 browser child has no --remote-debugging-port",
    );
    let error = webview2_background_click_refusal_error(
        click_target_kind_label(&point_target()),
        &cdp,
        0x25626122,
        0x131304,
    );
    assert_eq!(
        error_code(&error),
        Some(error_codes::A11Y_CDP_UNREACHABLE.to_owned())
    );
    let message = error.message.to_string();
    assert!(message.contains("point"), "{message}");
    assert!(message.contains("0x25626122"), "{message}");
    assert!(message.contains("0x131304"), "{message}");
    assert!(message.contains("cdp_debug=true"), "{message}");
}

#[test]
fn must_fail_point_click_refused_with_cdp_guidance_when_cdp_already_reachable() {
    // A reachable CDP endpoint exists but the point-click surface has no CDP
    // dispatch path (only the raw-coordinate/element CDP routes do); refuse
    // with routing guidance instead of falling through to PostMessage.
    let cdp = CdpDiagnostics {
        process_name: "FinanceManager.exe".to_owned(),
        status: CdpStatus::Ok,
        endpoint: Some("http://127.0.0.1:9227".to_owned()),
        checked_ports: vec![9227],
        checked_endpoints: vec!["http://127.0.0.1:9227".to_owned()],
        reason_code: None,
        detail: None,
        capabilities: Vec::new(),
        attached_node_count: None,
        selected_target_id: None,
        selected_session_id: None,
        target_selection_reason: None,
        target_candidate_count: None,
        frame_tree_frame_count: None,
        attached_frame_target_count: None,
        blocked_frame_targets: Vec::new(),
        frame_snapshot_errors: Vec::new(),
    };
    let error = webview2_background_click_refusal_error(
        click_target_kind_label(&point_target()),
        &cdp,
        0x25626122,
        0x131304,
    );
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_TARGET_INVALID.to_owned())
    );
    let message = error.message.to_string();
    assert!(message.contains("CDP element id"), "{message}");
    assert!(
        message.contains("target-scoped CDP coordinate click"),
        "{message}"
    );
}
