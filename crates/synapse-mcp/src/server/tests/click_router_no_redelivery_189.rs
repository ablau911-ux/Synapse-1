//! #189 regression: the verified `act_click` router must not re-deliver after a
//! tier already delivered. A tier that reports delivered (UIA Invoke/Toggle/
//! Select, or a PostMessage click) whose effect is only visible after
//! `verify_timeout_ms` used to be treated as "never ran", so the same click was
//! sent again through the next tier (up to three activations of one button).
//! Escalation is only legitimate when the previous tier did NOT deliver.

use super::*;

fn no_delta_error(tier_attempts: Vec<ActClickTierAttempt>) -> ErrorData {
    attach_click_tier_attempts(
        source_no_observed_delta_error(
            "act_click",
            "target_window_ui_or_pixels",
            250,
            "before".to_owned(),
            "after".to_owned(),
            json!({}),
        ),
        tier_attempts,
    )
}

fn delivered_then_unverified(tier: &str) -> Vec<ActClickTierAttempt> {
    vec![
        click_tier_delivered(tier, false, "delivered"),
        click_tier_failed(
            tier,
            "no_observed_delta",
            error_codes::ACTION_NO_OBSERVED_DELTA,
            false,
            "no delta within verify_timeout_ms",
        ),
    ]
}

#[test]
fn must_pass_delivered_no_delta_error_reports_delivered_true_and_keeps_the_code() {
    let error = mark_input_delivered(no_delta_error(delivered_then_unverified("uia")));
    let data = error.data.as_ref().expect("error data");
    assert_eq!(
        data.get("code").and_then(Value::as_str),
        Some(error_codes::ACTION_NO_OBSERVED_DELTA)
    );
    assert_eq!(data.get("delivered").and_then(Value::as_bool), Some(true));
}

fn checkbox(control_state: &str, focused: bool) -> synapse_core::AccessibleNode {
    synapse_core::AccessibleNode {
        element_id: synapse_core::ElementId::parse("0x1:0000002a00000001")
            .unwrap_or_else(|error| panic!("element id: {error}")),
        parent: None,
        name: "Remember me".to_owned(),
        role: "check box".to_owned(),
        automation_id: None,
        value: None,
        bbox: Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        },
        enabled: true,
        focused,
        patterns: Vec::new(),
        children_count: 0,
        depth: 1,
        control_state: Some(control_state.to_owned()),
    }
}

#[test]
fn must_pass_a_toggle_flip_with_focus_move_changes_the_effect_fingerprint() {
    let before = elements_fingerprint_hash(&[checkbox("toggle=0", false)], false);
    let after = elements_fingerprint_hash(&[checkbox("toggle=1", true)], false);
    assert_ne!(before.ok(), after.ok());
}

#[test]
fn must_fail_focus_alone_does_not_change_the_effect_fingerprint() {
    let before = elements_fingerprint_hash(&[checkbox("toggle=0", false)], false);
    let after = elements_fingerprint_hash(&[checkbox("toggle=0", true)], false);
    assert_eq!(before.ok(), after.ok());
}

#[test]
fn must_pass_selection_and_expansion_changes_affect_click_fingerprint() {
    for (before, after) in [
        ("selected=false", "selected=true"),
        ("expand=Collapsed", "expand=Expanded"),
    ] {
        let before =
            elements_fingerprint_hash(&[checkbox(before, false)], false).expect("before hash");
        let after = elements_fingerprint_hash(&[checkbox(after, true)], false).expect("after hash");
        assert_ne!(
            before, after,
            "#189: semantic state change is a click effect"
        );
    }
}

#[test]
fn must_fail_focus_without_selection_or_expansion_change_is_no_effect() {
    for state in [
        "selected=false",
        "selected=true",
        "expand=Collapsed",
        "expand=Expanded",
    ] {
        let before =
            elements_fingerprint_hash(&[checkbox(state, false)], false).expect("before hash");
        let after = elements_fingerprint_hash(&[checkbox(state, true)], false).expect("after hash");
        assert_eq!(
            before, after,
            "#107/#189: focus alone cannot justify delivery"
        );
    }
}
