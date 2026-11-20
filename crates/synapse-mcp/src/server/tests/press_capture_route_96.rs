//! #96: an explicit foreground key chord (`act operation=foreground`,
//! verb=key/key_chord) must reach delivery without a broad UIA snapshot: the
//! before/after evidence is the native foreground window (Win32 + window
//! pixels). A hung WebView2 provider then cannot block the chord.
//!
//! NEEDS-SEAM (fixer adds in `server/m2_tools.rs`; `act_press` uses it in place
//! of its inline `native_window_readback` / `verify_delta` choice, behaviour
//! unchanged):
//!   #[derive(Clone, Copy, Debug, Eq, PartialEq)]
//!   pub(super) enum ActPressBeforeCapture { None, NativeForeground, Semantic }
//!   pub(super) fn act_press_before_capture(params: &crate::m2::ActPressParams)
//!       -> ActPressBeforeCapture

use serde_json::json;

use super::*;
use crate::server::m2_tools::{ActPressBeforeCapture, act_press_before_capture};

#[test]
fn must_pass_foreground_key_chord_uses_native_foreground_evidence_only() {
    let params = target_act_foreground_press_params(
        vec!["ctrl".to_owned(), "shift".to_owned(), "i".to_owned()],
        None,
        "key_chord",
    )
    .expect("foreground key chord params");
    assert!(!params.verify_delta, "no broad UIA delta capture");
    assert!(params.force_foreground_key_delivery);
    assert_eq!(
        act_press_before_capture(&params),
        ActPressBeforeCapture::NativeForeground
    );
}

#[test]
fn must_pass_normal_target_scoped_chord_keeps_semantic_verification() {
    let params = target_act_press_params(vec!["ctrl".to_owned(), "c".to_owned()], None, "key")
        .expect("normal key params");
    assert!(params.verify_delta);
    assert!(!params.force_foreground_key_delivery);
    assert_eq!(
        act_press_before_capture(&params),
        ActPressBeforeCapture::Semantic
    );
}

#[test]
fn must_fail_unverified_plain_press_captures_nothing() {
    let params: crate::m2::ActPressParams =
        serde_json::from_value(json!({"keys": ["a"], "verify_delta": false}))
            .expect("plain press params");
    assert_eq!(
        act_press_before_capture(&params),
        ActPressBeforeCapture::None
    );
}

#[test]
fn must_fail_callers_cannot_select_the_foreground_lane_marker() {
    let refused = serde_json::from_value::<crate::m2::ActPressParams>(json!({
        "keys": ["a"],
        "force_foreground_key_delivery": true,
    }));
    let error = refused.expect_err("the private route marker is not a request field");
    assert!(
        error.to_string().contains("force_foreground_key_delivery"),
        "{error}"
    );
}

#[test]
fn must_fail_foreground_chord_with_printable_text_sequence_is_still_refused() {
    let error =
        target_act_foreground_press_params(vec!["p".to_owned(), "w".to_owned()], None, "key_chord")
            .expect_err("two printable keys are text, not a chord");
    assert_eq!(
        error.data.as_ref().and_then(|data| data["code"].as_str()),
        Some(error_codes::TOOL_PARAMS_INVALID)
    );
}

/// Wiring: act_press takes the decision from the seam, and the native capture
/// reads only the Win32 foreground + that window's pixels.
#[test]
fn must_pass_act_press_and_native_capture_avoid_the_broad_snapshot() {
    let source = include_str!("../m2_tools.rs");
    let start = source.find("pub async fn act_press(").expect("handler");
    let body = &source[start..];
    let body = &body[..body.find("\n    #[tool(").unwrap_or(body.len())];
    assert!(
        body.contains("act_press_before_capture("),
        "act_press must use the seam"
    );

    let start = source
        .find("fn capture_native_foreground_signature(")
        .expect("native capture");
    let native = &source[start..];
    // m2_tools.rs may be checked out with LF or CRLF line endings.
    let end = ["\n    }\n", "\n    }\r\n"]
        .iter()
        .filter_map(|closing| native.find(closing))
        .min()
        .expect("function end");
    let native = &native[..end];
    assert!(native.contains("synapse_a11y::current_foreground_context()"));
    assert!(native.contains("capture_coordinate_click_delta_signature("));
    assert!(!native.contains("capture_action_delta_signature("));
}
