//! What a UIA pattern click proved about the control (#107 part b).

use serde_json::{Value, json};
use synapse_a11y::ElementClickAction;
use synapse_core::ElementId;

#[cfg(test)]
#[path = "native_click_evidence_table_107b.rs"]
mod native_click_evidence_table_107b;

/// The evidence of one click that went through the UIA pattern chain: what
/// the chain did and whether the pattern's own state read proves the click.
pub(crate) struct PatternClickEvidence {
    action: ElementClickAction,
}

impl PatternClickEvidence {
    /// Takes what `synapse_a11y::click_element_action` returned.
    pub(crate) const fn of(action: ElementClickAction) -> Self {
        Self { action }
    }

    /// True when the pattern read a state that proves the click.
    pub(super) fn verified(&self) -> bool {
        match &self.action {
            ElementClickAction::Invoked | ElementClickAction::LegacyDefaultAction { .. } => false,
            ElementClickAction::Toggled {
                before_state,
                after_state,
            } => before_state != after_state,
            ElementClickAction::Selected { is_selected, .. } => *is_selected,
            ElementClickAction::Expanded {
                before_state,
                after_state,
            }
            | ElementClickAction::Collapsed {
                before_state,
                after_state,
            } => before_state != after_state,
        }
    }

    /// The name of the evidence.
    pub(crate) const fn source_of_truth(&self) -> &'static str {
        match &self.action {
            ElementClickAction::Invoked => "uia_invoke_pattern.invoke_returned",
            ElementClickAction::Toggled { .. } => "uia_toggle_pattern.toggle_state",
            ElementClickAction::Selected { .. } => "uia_selection_item_pattern.is_selected",
            ElementClickAction::Expanded { .. } | ElementClickAction::Collapsed { .. } => {
                "uia_expand_collapse_pattern.expand_collapse_state"
            }
            ElementClickAction::LegacyDefaultAction { .. } => {
                "uia_legacy_accessible_pattern.do_default_action_returned"
            }
        }
    }

    /// The `act_click` postcondition of a point click (#107 part d): the
    /// pattern's own state read when it proves the click, else `None` (the
    /// point check decides).
    pub(crate) fn postcondition(&self) -> Option<crate::m2::ActClickPostcondition> {
        self.verified().then(|| crate::m2::ActClickPostcondition {
            status: "verified_state".to_owned(),
            observed_delta: None,
            source_of_truth: Some(self.source_of_truth().to_owned()),
            before_signature: None,
            after_signature: None,
            detail: Some(format!(
                "act_click the UI Automation pattern read {} and it proves the click",
                self.source_of_truth()
            )),
            related_window_observed: None,
        })
    }

    /// The `target_act` result of this click.
    pub(super) fn reply(&self, window_hwnd: i64, element_id: &ElementId) -> Value {
        let source_of_truth = self.source_of_truth();
        let mut reply = json!({
            "delivery": "uia_click_pattern_chain",
            "source_of_truth": source_of_truth,
            "window_hwnd": window_hwnd,
            "element_id": element_id,
            "action": &self.action,
        });
        if self.verified() {
            reply["postcondition"] = json!({
                "status": "verified_state",
                "source_of_truth": source_of_truth,
            });
        }
        reply
    }
}
