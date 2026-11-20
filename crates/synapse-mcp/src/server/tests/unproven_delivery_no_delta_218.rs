//! #218: `verify_captured_action_delta` marks a no-delta error `delivered:true`
//! only when the input verifiably reached the target (SendInput / click paths).
//! The visual fallbacks after a posted key or after a failed act_type readback
//! go through `verify_captured_unproven_action_delta`, which never claims it.

use super::m2_tools_source_scan::method_body as fn_body;
use super::*;

fn unchanged() -> (ClickDeltaSignature, ClickDeltaSignature) {
    let before = tests::click_signature(100, 10, "app.exe", "App", 1);
    (before.clone(), before)
}

fn error_data(result: Result<ActPostcondition, ErrorData>) -> serde_json::Value {
    let Err(error) = result else {
        panic!("an unchanged signature must not verify")
    };
    let Some(data) = error.data else {
        panic!("structured error required")
    };
    assert_eq!(data["code"], error_codes::ACTION_NO_OBSERVED_DELTA);
    data
}

#[test]
fn must_pass_delivered_path_carries_delivered_and_retry_hint() {
    let (before, after) = unchanged();
    let data = error_data(verify_captured_action_delta(
        "act_click",
        "target_window_ui_or_pixels",
        250,
        before,
        after,
        None,
        ForegroundChangePolicy::reject(),
    ));
    assert_eq!(data["delivered"], true);
    assert_eq!(
        data["retry_hint"],
        crate::m2::postcondition::DELIVERED_NO_DELTA_RETRY_HINT
    );
}

#[test]
fn must_fail_unproven_path_carries_no_delivered_flag() {
    let (before, after) = unchanged();
    let data = error_data(verify_captured_unproven_action_delta(
        "act_press",
        "target_window_ui_or_pixels",
        250,
        before,
        after,
        None,
        ForegroundChangePolicy::reject(),
    ));
    assert!(data.get("delivered").is_none(), "{data}");
    assert!(data.get("retry_hint").is_none(), "{data}");
}

#[test]
fn must_fail_unproven_point_region_carries_no_delivered_flag() {
    let (before, after) = unchanged();
    let data = error_data(verify_captured_unproven_action_delta(
        "act_type",
        "target_window_ui_or_pixels",
        250,
        before,
        after,
        Some(Point { x: 5, y: 5 }),
        ForegroundChangePolicy::reject(),
    ));
    assert!(data.get("delivered").is_none(), "{data}");
}

#[test]
fn must_fail_posted_key_visual_fallback_uses_the_unproven_verifier() {
    let body = fn_body("act_press_postmessage_background_target");
    assert!(
        body.contains("verify_captured_unproven_action_delta("),
        "posted-key visual fallback must not claim delivery"
    );
    assert!(!body.contains("verify_captured_action_delta("));
}

#[test]
fn must_fail_act_type_visual_fallback_uses_the_unproven_verifier() {
    let body = fn_body("verify_act_type_visual_delta_response");
    assert!(
        body.contains("verify_captured_unproven_action_delta("),
        "act_type readback fallback must not claim delivery"
    );
    assert!(!body.contains("verify_captured_action_delta("));
}

#[test]
fn must_pass_send_input_paths_keep_the_delivered_verifier() {
    let body = fn_body("verify_action_delta");
    assert!(body.contains("verify_captured_action_delta("));
}
