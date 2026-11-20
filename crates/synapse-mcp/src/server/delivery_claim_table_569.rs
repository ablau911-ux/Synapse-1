//! FROZEN behaviour table of Gitea #2340 (roadmap item #569), rows L01-L41.
//! Written by the manager from the issue table before the code. Coders never
//! edit this file; a wrong row is reported to the manager with evidence.

use serde_json::{Value, json};

use super::DeliveryClaim;

const VERIFIED: &str = "delivered_and_verified";
const PIXEL: &str = "delivered_and_pixel_verified";
const UNVERIFIED: &str = "delivered_unverified";

const UIA_TEXT: &str = "foreground_text_readback:uia_focused_value";
const WINDOW_CHANGED: &str = "target_window_ui_or_pixels";
const FOREGROUND_CHANGED: &str = "foreground_focused_ui_or_pixels";
const RELATED_WINDOW: &str = "foreground_related_window";

/// `{"status", "observed_delta", "source_of_truth"}` as the routes record it.
fn evidence(status: &str, observed: bool, source_of_truth: &str) -> Value {
    json!({
        "status": status,
        "observed_delta": observed,
        "source_of_truth": source_of_truth,
        "before_signature": "before",
        "after_signature": "after",
        "detail": "table row",
    })
}

/// A result with the evidence at the top level.
fn pc(status: &str, observed: bool, source_of_truth: &str) -> Value {
    json!({ "postcondition": evidence(status, observed, source_of_truth) })
}

fn not_requested() -> Value {
    json!({ "status": "not_requested", "source_of_truth": UIA_TEXT })
}

fn valid_pixel_evidence() -> Value {
    json!({
        "status": "observed_delta",
        "observed_delta": true,
        "source_of_truth": "target_point_pixels",
        "before_signature": "a".repeat(64),
        "after_signature": "b".repeat(64),
    })
}

fn step(name: &str, postcondition: Value) -> Value {
    json!({ "step": name, "result": { "postcondition": postcondition } })
}

fn steps(steps: Vec<Value>) -> Value {
    json!({ "ok": true, "mode": "typed", "steps": steps })
}

#[test]
fn delivery_claim_table_569() {
    let rows: Vec<(&str, &str, bool, &str, Value, &str)> = vec![
        (
            "L01_type_exact_text_read_through_uia",
            "type",
            true,
            "ok",
            pc("observed_delta", true, UIA_TEXT),
            VERIFIED,
        ),
        (
            "L02_type_exact_text_read_from_the_bridge_element",
            "type",
            true,
            "ok",
            pc("observed_delta", true, "chrome_bridge_active_element.value"),
            VERIFIED,
        ),
        (
            "L03_type_exact_text_read_through_ocr",
            "type",
            true,
            "ok",
            pc(
                "observed_delta",
                true,
                "foreground_text_readback:ocr_focused_rect_text",
            ),
            VERIFIED,
        ),
        (
            "L04_set_field_exact_value_through_uia",
            "set_field",
            true,
            "ok",
            pc("verified_state", true, "uia_value_pattern.value"),
            VERIFIED,
        ),
        (
            "L05_set_field_value_was_already_there",
            "set_field",
            true,
            "ok",
            pc("verified_state", false, "win32_window_text"),
            VERIFIED,
        ),
        (
            "L06_set_field_password_length",
            "set_field",
            true,
            "ok",
            pc("verified_state", true, "uia_value_pattern.password_length"),
            VERIFIED,
        ),
        (
            "L07_type_anything_in_the_window_changed",
            "type",
            true,
            "ok",
            pc("observed_delta", true, WINDOW_CHANGED),
            PIXEL,
        ),
        (
            "L08_type_anything_in_the_foreground_changed",
            "type",
            true,
            "ok",
            pc("observed_delta", true, FOREGROUND_CHANGED),
            PIXEL,
        ),
        (
            "L09_key_anything_in_the_window_changed",
            "key",
            true,
            "ok",
            pc("observed_delta", true, WINDOW_CHANGED),
            PIXEL,
        ),
        (
            "L10_key_chord_anything_in_the_foreground_changed",
            "key_chord",
            true,
            "ok",
            pc("observed_delta", true, FOREGROUND_CHANGED),
            PIXEL,
        ),
        (
            "L11_press_anything_in_the_window_changed",
            "press",
            true,
            "ok",
            pc("observed_delta", true, WINDOW_CHANGED),
            PIXEL,
        ),
        (
            "L12_insert_text_typed_step_saw_only_a_window_change",
            "insert_text",
            true,
            "ok",
            steps(vec![
                json!({ "step": "focus", "result": {} }),
                step("type", evidence("observed_delta", true, WINDOW_CHANGED)),
            ]),
            PIXEL,
        ),
        (
            "L13_type_another_window_appeared_is_not_the_text",
            "type",
            true,
            "ok",
            pc("observed_delta", true, RELATED_WINDOW),
            UNVERIFIED,
        ),
        (
            "L14_append_text_typed_step_saw_only_another_window",
            "append_text",
            true,
            "ok",
            steps(vec![
                step(
                    "move_caret_to_end",
                    evidence("observed_delta", true, "target_hwnd_text_or_selection"),
                ),
                step("type", evidence("observed_delta", true, RELATED_WINDOW)),
            ]),
            UNVERIFIED,
        ),
        (
            "L15_neighbour_click_that_opened_a_window_stays_verified",
            "click",
            true,
            "ok",
            pc("observed_delta", true, RELATED_WINDOW),
            VERIFIED,
        ),
        (
            "L16_neighbour_key_with_a_related_window_stays_verified",
            "key",
            true,
            "ok",
            pc("observed_delta", true, RELATED_WINDOW),
            VERIFIED,
        ),
        (
            "L17_set_field_bridge_verdict_at_result_status",
            "set_field",
            true,
            "ok",
            json!({
                "status": "verified_state",
                "source_of_truth": "chrome_bridge_in_page_value + separate chrome.tabs active-element readback",
            }),
            VERIFIED,
        ),
        (
            "L18_set_field_plain_ok_without_evidence",
            "set_field",
            true,
            "ok",
            json!({ "status": "ok" }),
            UNVERIFIED,
        ),
        (
            "L19_insert_text_native_edit_lengths_equal",
            "insert_text",
            true,
            "ok",
            json!({
                "method": "native_edit_em_replacesel",
                "after_text_utf16_len": 7,
                "expected_after_text_utf16_len": 7,
            }),
            VERIFIED,
        ),
        (
            "L20_insert_text_native_edit_lengths_differ",
            "insert_text",
            true,
            "ok",
            json!({
                "method": "native_edit_em_replacesel",
                "after_text_utf16_len": 7,
                "expected_after_text_utf16_len": 9,
            }),
            UNVERIFIED,
        ),
        (
            "L21_append_text_native_edit_one_length_missing",
            "append_text",
            true,
            "ok",
            json!({
                "method": "native_edit_em_setsel_em_replacesel",
                "after_text_utf16_len": 7,
            }),
            UNVERIFIED,
        ),
        (
            "L22_insert_text_typed_step_exact_text",
            "insert_text",
            true,
            "ok",
            steps(vec![
                json!({ "step": "focus", "result": {} }),
                step("type", evidence("observed_delta", true, UIA_TEXT)),
            ]),
            VERIFIED,
        ),
        (
            "L23_append_text_only_the_type_step_speaks_for_the_text",
            "append_text",
            true,
            "ok",
            steps(vec![
                step(
                    "move_caret_to_end",
                    evidence("observed_delta", true, "target_hwnd_text_or_selection"),
                ),
                step("type", not_requested()),
            ]),
            UNVERIFIED,
        ),
        (
            "L24_type_at_a_point_typing_evidence_is_nested",
            "type",
            true,
            "ok",
            json!({
                "coordinate_delegated_tool": "act_click",
                "coordinate_click": { "postcondition": valid_pixel_evidence() },
                "type": { "postcondition": evidence("observed_delta", true, UIA_TEXT) },
            }),
            VERIFIED,
        ),
        (
            "L25_type_at_a_point_the_click_never_speaks_for_the_text",
            "type",
            true,
            "ok",
            json!({
                "coordinate_delegated_tool": "act_click",
                "coordinate_click": { "postcondition": valid_pixel_evidence() },
                "type": { "postcondition": not_requested() },
            }),
            UNVERIFIED,
        ),
        (
            "L26_neighbour_click_with_a_bare_result_status",
            "click",
            true,
            "ok",
            json!({ "status": "verified_state" }),
            UNVERIFIED,
        ),
        (
            "L27_neighbour_click_ignores_nested_typing_evidence",
            "click",
            true,
            "ok",
            steps(vec![step(
                "type",
                evidence("observed_delta", true, UIA_TEXT),
            )]),
            UNVERIFIED,
        ),
        (
            "L28_neighbour_click_point_pixels_changed",
            "click",
            true,
            "ok",
            json!({ "postcondition": valid_pixel_evidence() }),
            PIXEL,
        ),
        (
            "L29_click_window_change_is_pixel_verified_2367",
            "click",
            true,
            "ok",
            pc("observed_delta", true, WINDOW_CHANGED),
            PIXEL,
        ),
        (
            "L30_scroll_without_evidence",
            "scroll",
            true,
            "ok",
            json!({ "wheel_event_count": 1 }),
            UNVERIFIED,
        ),
        (
            "L31_type_check_not_requested",
            "type",
            true,
            "ok",
            json!({ "postcondition": not_requested() }),
            UNVERIFIED,
        ),
        (
            "L32_type_observed_delta_is_false",
            "type",
            true,
            "ok",
            pc("observed_delta", false, UIA_TEXT),
            UNVERIFIED,
        ),
        (
            "L33_type_verify_needed_with_exact_evidence",
            "type",
            false,
            "verify_needed",
            pc("observed_delta", true, UIA_TEXT),
            UNVERIFIED,
        ),
        (
            "L34_type_refused_with_exact_evidence",
            "type",
            false,
            "refused",
            pc("observed_delta", true, UIA_TEXT),
            "refused_before_delivery",
        ),
        (
            "L35_key_deferred",
            "key",
            false,
            "deferred",
            json!({}),
            "deferred_without_delivery",
        ),
        (
            "L36_type_error",
            "type",
            false,
            "error",
            json!({}),
            "failed_before_delivery",
        ),
        (
            "L37_type_result_is_null",
            "type",
            true,
            "ok",
            Value::Null,
            UNVERIFIED,
        ),
        (
            "L38_type_postcondition_has_the_wrong_shape",
            "type",
            true,
            "ok",
            json!({ "postcondition": "verified_state" }),
            UNVERIFIED,
        ),
        (
            "L39a_insert_text_steps_is_a_string",
            "insert_text",
            true,
            "ok",
            json!({ "ok": true, "steps": "type" }),
            UNVERIFIED,
        ),
        (
            "L39b_insert_text_steps_is_empty",
            "insert_text",
            true,
            "ok",
            steps(vec![]),
            UNVERIFIED,
        ),
        (
            "L39c_insert_text_steps_has_no_type_step",
            "insert_text",
            true,
            "ok",
            steps(vec![step(
                "focus",
                evidence("observed_delta", true, UIA_TEXT),
            )]),
            UNVERIFIED,
        ),
        (
            "L40_neighbour_key_text_or_selection_changed",
            "key",
            true,
            "ok",
            pc("observed_delta", true, "target_hwnd_text_or_selection"),
            VERIFIED,
        ),
        (
            "L41_neighbour_key_cdp_value_or_selection_changed",
            "key",
            true,
            "ok",
            pc(
                "observed_delta",
                true,
                "cdp_active_element_value_or_selection",
            ),
            VERIFIED,
        ),
        (
            "L42_dblclick_window_change_is_pixel_verified_2367",
            "dblclick",
            true,
            "ok",
            pc("observed_delta", true, WINDOW_CHANGED),
            PIXEL,
        ),
        (
            "L43_select_window_change_is_pixel_verified_2367",
            "select",
            true,
            "ok",
            pc("observed_delta", true, WINDOW_CHANGED),
            PIXEL,
        ),
        (
            "L44_click_state_read_back_stays_verified_2367",
            "click",
            true,
            "ok",
            pc("verified_state", true, WINDOW_CHANGED),
            VERIFIED,
        ),
    ];

    let mut failures = Vec::new();
    for (row, verb, ok, status, result, expected) in &rows {
        let observed = DeliveryClaim::new(verb, *ok, status, result).state();
        if observed != *expected {
            failures.push(format!("{row}: expected {expected}, observed {observed}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
