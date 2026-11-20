//! FROZEN behaviour table of #107 part (b), rows E1-E11 (Gitea #107, comment of
//! 2026-11-19 20:35). Written by the manager from the issue table before the
//! code. Coders never edit this file; a wrong row is reported to the manager.

use serde_json::json;
use synapse_a11y::{ElementClickAction, ExpandState};
use synapse_core::ElementId;

use super::PatternClickEvidence;
use crate::server::delivery_claim::DeliveryClaim;

const HWND: i64 = 0x1234;
const NATIVE_ID: &str = "0x1234:2a0001";

const INVOKE: &str = "uia_invoke_pattern.invoke_returned";
const TOGGLE: &str = "uia_toggle_pattern.toggle_state";
const SELECT: &str = "uia_selection_item_pattern.is_selected";
const EXPAND: &str = "uia_expand_collapse_pattern.expand_collapse_state";
const LEGACY: &str = "uia_legacy_accessible_pattern.do_default_action_returned";

fn native_id() -> ElementId {
    match ElementId::parse(NATIVE_ID) {
        Ok(id) => id,
        Err(error) => panic!("table setup: element id: {error:?}"),
    }
}

fn toggled(before: &str, after: &str) -> ElementClickAction {
    ElementClickAction::Toggled {
        before_state: before.to_owned(),
        after_state: after.to_owned(),
    }
}

#[test]
fn native_click_evidence_table_107b() {
    let rows: Vec<(&str, ElementClickAction, bool, &str)> = vec![
        (
            "E1_invoked_reads_nothing",
            ElementClickAction::Invoked,
            false,
            INVOKE,
        ),
        ("E2_toggled_off_to_on", toggled("Off", "On"), true, TOGGLE),
        (
            "E3_toggled_but_state_stayed",
            toggled("On", "On"),
            false,
            TOGGLE,
        ),
        (
            "E4_selected_now",
            ElementClickAction::Selected {
                was_selected: false,
                is_selected: true,
            },
            true,
            SELECT,
        ),
        (
            "E5_radio_already_selected",
            ElementClickAction::Selected {
                was_selected: true,
                is_selected: true,
            },
            true,
            SELECT,
        ),
        (
            "E6_not_selected_afterwards",
            ElementClickAction::Selected {
                was_selected: false,
                is_selected: false,
            },
            false,
            SELECT,
        ),
        (
            "E7_expanded",
            ElementClickAction::Expanded {
                before_state: ExpandState::Collapsed,
                after_state: ExpandState::Expanded,
            },
            true,
            EXPAND,
        ),
        (
            "E8_expand_changed_nothing",
            ElementClickAction::Expanded {
                before_state: ExpandState::LeafNode,
                after_state: ExpandState::LeafNode,
            },
            false,
            EXPAND,
        ),
        (
            "E9_collapsed",
            ElementClickAction::Collapsed {
                before_state: ExpandState::Expanded,
                after_state: ExpandState::Collapsed,
            },
            true,
            EXPAND,
        ),
        (
            "E10_legacy_default_action_reads_nothing",
            ElementClickAction::LegacyDefaultAction {
                default_action: Some("Press".to_owned()),
            },
            false,
            LEGACY,
        ),
    ];

    let element_id = native_id();
    let mut failures = Vec::new();
    for (row, action, verified, source_of_truth) in &rows {
        let evidence = PatternClickEvidence::of(action.clone());
        let reply = evidence.reply(HWND, &element_id);
        let word = DeliveryClaim::new("click", true, "ok", &reply).state();
        let expected_word = if *verified {
            "delivered_and_verified"
        } else {
            "delivered_unverified"
        };
        let expected_postcondition = if *verified {
            json!({ "status": "verified_state", "source_of_truth": source_of_truth })
        } else {
            serde_json::Value::Null
        };
        let mut check = |what: &str, ok: bool, observed: String| {
            if !ok {
                failures.push(format!("{row}: {what}: observed {observed}"));
            }
        };
        check(
            "verified()",
            evidence.verified() == *verified,
            evidence.verified().to_string(),
        );
        check(
            "source_of_truth()",
            evidence.source_of_truth() == *source_of_truth,
            evidence.source_of_truth().to_owned(),
        );
        // E11: the reply names the window, the element, the delivery and the action.
        check(
            "reply.window_hwnd",
            reply.get("window_hwnd") == Some(&json!(HWND)),
            reply.to_string(),
        );
        check(
            "reply.element_id",
            reply.get("element_id") == Some(&json!(&element_id)),
            reply.to_string(),
        );
        check(
            "reply.delivery",
            reply.get("delivery") == Some(&json!("uia_click_pattern_chain")),
            reply.to_string(),
        );
        check(
            "reply.source_of_truth",
            reply.get("source_of_truth") == Some(&json!(source_of_truth)),
            reply.to_string(),
        );
        check(
            "reply.action",
            reply.get("action") == Some(&json!(action)),
            reply.to_string(),
        );
        check(
            "reply.postcondition",
            reply.get("postcondition").cloned().unwrap_or_default() == expected_postcondition,
            reply.to_string(),
        );
        check("delivery word", word == expected_word, word.to_owned());
    }
    assert!(
        failures.is_empty(),
        "{} checks failed in {} rows:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
