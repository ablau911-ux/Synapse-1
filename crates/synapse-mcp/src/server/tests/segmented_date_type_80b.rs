//! #80 (UIA-only path): typing a full date into a native `<input type=date>`
//! moves focus between month/day/year spin buttons. Verification must use the
//! shared parent date control, not fail on the focused-segment change.

use super::act_type_date_parent::{date_parent_readback, date_text_matches_typed};
use super::*;

#[path = "type_postcondition_281.rs"]
mod type_postcondition_281;

const HWND: i64 = 0x1000;

fn id(runtime: u32) -> String {
    format!("0x{HWND:x}:{runtime:08x}")
}

fn segment_signature(segment_runtime: u32) -> ActTypeTextSignature {
    ActTypeTextSignature {
        foreground_hwnd: HWND,
        foreground_pid: 77,
        foreground_process: "webview-test.exe".to_owned(),
        focused_element_id: Some(id(segment_runtime)),
        focused_role: Some("spin button".to_owned()),
        readback_source: Some(ACT_TYPE_TEXT_SOURCE_UIA_VALUE.to_owned()),
        has_text_readback: true,
        ..Default::default()
    }
}

fn readback(
    segment_runtime: u32,
    parent_runtime: Option<u32>,
    parent_value: Option<&str>,
    segments: &[&str],
) -> ActTypeTextReadback {
    ActTypeTextReadback {
        signature: segment_signature(segment_runtime),
        value: segments.first().map(|segment| (*segment).to_owned()),
        browser_url: None,
        date_parent: parent_runtime.map(|runtime| {
            super::act_type_date_parent::ActTypeDateParentReadback {
                parent_id: id(runtime),
                parent_value: parent_value.map(str::to_owned),
                segment_values: segments
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect(),
            }
        }),
    }
}

fn response() -> ActTypeResponse {
    ActTypeResponse {
        ok: true,
        chars_typed: 8,
        elapsed_ms: 10,
        backend_tier_used: "foreground".to_owned(),
        required_foreground: true,
        target_text_integrity: "dispatch_only_requires_target_readback".to_owned(),
        target_readback_required: true,
        minimum_linear_ms_per_char: 20,
        postcondition: crate::m2::postcondition::postcondition_not_requested(
            "act_type",
            "foreground_focused_ui_or_pixels",
        ),
    }
}

fn verify(
    before: ActTypeTextReadback,
    after: ActTypeTextReadback,
) -> Result<ActTypeResponse, ErrorData> {
    verify_act_type_text_response(
        response(),
        before,
        after,
        "before".to_owned(),
        "after".to_owned(),
        250,
        "09122026",
    )
}

fn error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

#[test]
fn must_pass_segment_focus_change_under_same_parent_is_verified_by_parent_value() {
    let before = readback(1, Some(100), Some("mm/dd/yyyy"), &["mm", "dd", "yyyy"]);
    let after = readback(3, Some(100), Some("09/12/2026"), &["09", "12", "2026"]);
    assert!(act_type_text_target_changed(
        &before.signature,
        &after.signature
    ));
    assert!(!act_type_text_terminal_failure(&before, &after));

    let verified = verify(before, after).expect("same-parent date segments must verify");
    assert_eq!(verified.postcondition.status, "observed_delta");
    assert!(!verified.target_readback_required);
    assert_eq!(
        verified.target_text_integrity,
        "verify_delta_text_readback:uia_date_parent"
    );
}

#[test]
fn must_pass_segment_focus_change_without_parent_value_uses_combined_segment_values() {
    let before = readback(1, Some(100), None, &["mm", "dd", "yyyy"]);
    let after = readback(3, Some(100), None, &["9", "12", "2026"]);
    let verified = verify(before, after).expect("combined segment values must verify");
    assert_eq!(verified.postcondition.status, "observed_delta");
    assert_eq!(verified.postcondition.observed_delta, Some(true));
    assert!(
        verified
            .postcondition
            .source_of_truth
            .as_deref()
            .is_some_and(|source| source.ends_with(":uia_date_parent")),
        "{:?}",
        verified.postcondition.source_of_truth
    );
    assert!(!verified.target_readback_required);
    assert_eq!(
        verified.target_text_integrity,
        "verify_delta_text_readback:uia_date_parent"
    );
}

#[test]
fn must_fail_unrelated_focus_change_still_fails() {
    let before = readback(1, Some(100), Some("mm/dd/yyyy"), &["mm", "dd", "yyyy"]);
    let after = readback(9, Some(200), Some("09/12/2026"), &["09", "12", "2026"]);
    assert!(act_type_text_terminal_failure(&before, &after));
    let error = verify(before, after).expect_err("different parents must not verify");
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_POSTCONDITION_FAILED)
    );
}

#[test]
fn must_fail_focus_change_without_any_parent_still_fails() {
    let before = readback(1, None, None, &[]);
    let after = readback(3, Some(100), Some("09/12/2026"), &["09", "12", "2026"]);
    let error = verify(before, after).expect_err("missing parent must not verify");
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_POSTCONDITION_FAILED)
    );
}

#[test]
fn must_fail_same_parent_with_a_different_date_fails() {
    let before = readback(1, Some(100), Some("mm/dd/yyyy"), &["mm", "dd", "yyyy"]);
    let after = readback(3, Some(100), Some("09/13/2026"), &["09", "13", "2026"]);
    let error = verify(before, after).expect_err("wrong date must not verify");
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_POSTCONDITION_FAILED)
    );
}

#[test]
fn must_fail_same_parent_without_a_value_change_reports_no_delta() {
    let before = readback(1, Some(100), Some("09/12/2026"), &["09", "12", "2026"]);
    let after = readback(3, Some(100), Some("09/12/2026"), &["09", "12", "2026"]);
    let error = verify(before, after).expect_err("no change must not verify");
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_NO_OBSERVED_DELTA)
    );
}

#[test]
fn must_pass_typed_digits_match_date_forms_only() {
    for text in [
        "09/12/2026",
        "9/12/2026",
        "2026-09-12",
        "09122026",
        "20260912",
    ] {
        assert!(date_text_matches_typed(text, "09122026"), "{text}");
    }
    for text in [
        "",
        "mm/dd/yyyy",
        "09/dd/yyyy",
        "09/13/2026",
        "2026-09-13",
        "2026",
    ] {
        assert!(!date_text_matches_typed(text, "09122026"), "{text}");
    }
    assert!(!date_text_matches_typed("09/12/2026", "0912202"));
    assert!(!date_text_matches_typed("09/12/2026", "09a22026"));
}

fn node(runtime: u32, parent: Option<u32>, role: &str, value: Option<&str>) -> AccessibleNode {
    AccessibleNode {
        element_id: ElementId::parse(&id(runtime)).expect("synthetic element id"),
        parent: parent.map(|runtime| ElementId::parse(&id(runtime)).expect("synthetic parent id")),
        name: String::new(),
        role: role.to_owned(),
        automation_id: None,
        value: value.map(str::to_owned),
        bbox: Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        },
        enabled: true,
        focused: false,
        patterns: Vec::new(),
        children_count: 0,
        depth: 0,
        control_state: None,
    }
}

#[test]
fn must_pass_parent_readback_is_built_only_for_spin_button_segments() {
    let elements = vec![
        node(100, None, "group", Some("09/12/2026")),
        node(1, Some(100), "spin button", Some("09")),
        node(2, Some(100), "spin button", Some("12")),
        node(3, Some(100), "spin button", Some("2026")),
        node(5, Some(100), "button", None),
        node(7, None, "edit", Some("text")),
    ];
    let parent = date_parent_readback(&id(3), &elements).expect("segment has a parent");
    assert_eq!(parent.parent_id, id(100));
    assert_eq!(parent.parent_value.as_deref(), Some("09/12/2026"));
    assert_eq!(parent.segment_values, ["09", "12", "2026"]);

    assert!(date_parent_readback(&id(7), &elements).is_none());
    assert!(date_parent_readback(&id(5), &elements).is_none());
    assert!(date_parent_readback(&id(404), &elements).is_none());
}

#[test]
fn must_pass_live_uia_parent_readback_drives_verification_when_the_snapshot_has_no_parent() {
    use super::act_type_date_parent::ActTypeDateParentReadback;

    let live = |segments: &[&str]| synapse_a11y::FocusedParentReadback {
        parent_id: id(100),
        parent_value: None,
        child_spin_values: segments.iter().map(|s| (*s).to_owned()).collect(),
    };
    let mut before = readback(1, None, None, &[]);
    before.date_parent = Some(ActTypeDateParentReadback::from_live(live(&[
        "9", "dd", "yyyy",
    ])));
    let mut after = readback(3, None, None, &[]);
    after.date_parent = Some(ActTypeDateParentReadback::from_live(live(&[
        "9", "12", "2026",
    ])));
    assert!(!act_type_text_terminal_failure(&before, &after));
    let verified = verify(before, after).expect("live same-parent segments must verify");
    assert_eq!(verified.postcondition.status, "observed_delta");
    assert_eq!(verified.postcondition.observed_delta, Some(true));
    assert!(!verified.target_readback_required);
    assert_eq!(
        verified.target_text_integrity,
        "verify_delta_text_readback:uia_date_parent"
    );
}

#[test]
fn must_fail_live_uia_parent_readback_of_a_different_parent_does_not_verify() {
    use super::act_type_date_parent::ActTypeDateParentReadback;

    let live = |parent: u32, segments: &[&str]| synapse_a11y::FocusedParentReadback {
        parent_id: id(parent),
        parent_value: None,
        child_spin_values: segments.iter().map(|s| (*s).to_owned()).collect(),
    };
    let mut before = readback(1, None, None, &[]);
    before.date_parent = Some(ActTypeDateParentReadback::from_live(live(
        100,
        &["9", "dd", "yyyy"],
    )));
    let mut other = readback(3, None, None, &[]);
    other.date_parent = Some(ActTypeDateParentReadback::from_live(live(
        200,
        &["9", "12", "2026"],
    )));
    assert!(act_type_text_terminal_failure(&before, &other));
    let error = verify(before, other).expect_err("different live parents must not verify");
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_POSTCONDITION_FAILED)
    );
}

#[test]
fn must_pass_spinner_control_type_counts_as_a_date_segment() {
    use super::act_type_date_parent::is_spin_button;
    for role in ["spin button", "Spinner", "spin_button", "SpinButton"] {
        assert!(is_spin_button(role), "{role}");
    }
    for role in ["edit", "group", "button", ""] {
        assert!(!is_spin_button(role), "{role}");
    }
}
