//! #79: a claimed-but-background click into a WebView2 host without a CDP
//! session must be refused BEFORE delivery (a synthetic PostMessage click is
//! not consumed by the Chromium renderer and silently reports delivered),
//! while native hosts, foreground WebView2 targets and CDP element ids keep
//! their routes.
//!
//! Contract: issue #79 comments 2026-09-12 (GLM scoping: refuse only the
//! background WebView2 renderer route without a proven CDP target; native host
//! regions stay eligible; never auto-escalate to foreground) and 2026-09-21
//! (no-CDP WebView2 coordinate click refuses before delivery with
//! `A11Y_CDP_UNREACHABLE`); server/m2_tools.rs
//! `webview2_background_click_refusal_error` (reachable CDP ->
//! `ACTION_TARGET_INVALID` with CDP routing guidance).
//!
//! Drives the production preflight `preflight_background_webview_uia_click_with`
//! (the body `act_click` runs through `preflight_background_webview_uia_click`)
//! with injected window facts, so no real WebView2 window is needed.
//! The successful owned-CDP background click (Dashboard -> Payments) is a
//! live VBV item (see criteria_079.md), not automatable here.

use std::cell::Cell;

use serde_json::json;
use synapse_core::{CdpDiagnostics, CdpStatus, error_codes};

use super::*;
use crate::server::g4_unit_support::error_code;

const TARGET_ROOT: i64 = 0x0025_6261;
const HUMAN_ROOT: i64 = 0x0013_1304;

fn point_click() -> ActClickParams {
    serde_json::from_value(json!({
        "target": {"x": 60, "y": 325},
        "verify_target_window_hwnd": TARGET_ROOT,
    }))
    .expect("point click params")
}

fn native_element_click() -> ActClickParams {
    serde_json::from_value(json!({"target": {"element_id": "0x256261:0000002a00000001"}}))
        .expect("element click params")
}

fn cdp_element_click() -> ActClickParams {
    let id = synapse_a11y::cdp_element_id(TARGET_ROOT, 42).to_string();
    serde_json::from_value(json!({"target": {"element_id": id}})).expect("cdp click params")
}

fn unreachable_cdp() -> CdpDiagnostics {
    CdpDiagnostics::unreachable_with_probe(
        "FinanceManager.exe",
        error_codes::A11Y_CDP_UNREACHABLE,
        Vec::new(),
        "WebView2 browser child has no --remote-debugging-port",
    )
}

fn reachable_cdp() -> CdpDiagnostics {
    CdpDiagnostics {
        status: CdpStatus::Ok,
        endpoint: Some("http://127.0.0.1:9227".to_owned()),
        checked_ports: vec![9227],
        ..unreachable_cdp()
    }
}

#[test]
fn must_fail_background_webview2_point_click_without_cdp_is_refused() {
    let error = preflight_background_webview_uia_click_with(
        &point_click(),
        |_| Ok(Some(TARGET_ROOT)),
        |_| Some(unreachable_cdp()),
        || Ok(HUMAN_ROOT),
    )
    .expect_err("background WebView2 point click without CDP must be refused");
    assert_eq!(
        error_code(&error).as_deref(),
        Some(error_codes::A11Y_CDP_UNREACHABLE)
    );
    let message = error.message.to_string();
    assert!(message.contains("before delivery"), "{message}");
    assert!(message.contains("point"), "{message}");
    assert!(message.contains("cdp_debug=true"), "{message}");
}

#[test]
fn must_fail_background_webview2_element_click_with_reachable_cdp_is_refused() {
    let error = preflight_background_webview_uia_click_with(
        &native_element_click(),
        |_| Ok(Some(TARGET_ROOT)),
        |_| Some(reachable_cdp()),
        || Ok(HUMAN_ROOT),
    )
    .expect_err("a UIA-element PostMessage click into WebView2 must be refused");
    assert_eq!(
        error_code(&error).as_deref(),
        Some(error_codes::ACTION_TARGET_INVALID)
    );
    assert!(error.message.contains("UIA element"), "{}", error.message);
}

#[test]
fn must_pass_webview2_target_that_is_foreground_keeps_its_route() {
    let result = preflight_background_webview_uia_click_with(
        &point_click(),
        |_| Ok(Some(TARGET_ROOT)),
        |_| Some(unreachable_cdp()),
        || Ok(TARGET_ROOT),
    );
    assert_eq!(
        result.map_err(|e| e.message.to_string()),
        Ok(()),
        "foreground WebView2 is not refused"
    );
}

#[test]
fn must_pass_native_host_never_reads_the_foreground() {
    let foreground_reads = Cell::new(0_u32);
    let result = preflight_background_webview_uia_click_with(
        &point_click(),
        |_| Ok(Some(TARGET_ROOT)),
        |_| None,
        || {
            foreground_reads.set(foreground_reads.get() + 1);
            Ok(HUMAN_ROOT)
        },
    );
    assert_eq!(
        result.map_err(|e| e.message.to_string()),
        Ok(()),
        "native host regions stay eligible"
    );
    assert_eq!(foreground_reads.get(), 0);
}

#[test]
fn must_pass_cdp_element_click_skips_every_probe() {
    let probes = Cell::new(0_u32);
    let result = preflight_background_webview_uia_click_with(
        &cdp_element_click(),
        |_| {
            probes.set(probes.get() + 1);
            Ok(Some(TARGET_ROOT))
        },
        |_| {
            probes.set(probes.get() + 1);
            Some(unreachable_cdp())
        },
        || {
            probes.set(probes.get() + 1);
            Ok(HUMAN_ROOT)
        },
    );
    assert_eq!(
        result.map_err(|e| e.message.to_string()),
        Ok(()),
        "CDP element ids go over CDP"
    );
    assert_eq!(probes.get(), 0, "the CDP route needs no window probes");
}

#[test]
fn must_fail_unreadable_foreground_fails_closed_for_a_webview2_host() {
    let error = preflight_background_webview_uia_click_with(
        &point_click(),
        |_| Ok(Some(TARGET_ROOT)),
        |_| Some(unreachable_cdp()),
        || {
            Err(crate::m1::mcp_error(
                error_codes::A11Y_NO_FOREGROUND,
                "GetForegroundWindow returned null",
            ))
        },
    )
    .expect_err("an unknown foreground must not let a WebView2 click through");
    assert_eq!(
        error_code(&error).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
}

#[test]
fn must_fail_target_root_resolution_error_is_returned_unchanged() {
    let error = preflight_background_webview_uia_click_with(
        &point_click(),
        |_| {
            Err(crate::m1::mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "verify_target_window_hwnd 0x256261 is gone",
            ))
        },
        |_| Some(unreachable_cdp()),
        || Ok(HUMAN_ROOT),
    )
    .expect_err("a dead target must not pass");
    assert_eq!(
        error_code(&error).as_deref(),
        Some(error_codes::ACTION_TARGET_INVALID)
    );
}
