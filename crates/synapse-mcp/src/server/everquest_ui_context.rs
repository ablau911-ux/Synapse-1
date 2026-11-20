use rmcp::{ErrorData, model::ErrorCode, schemars::JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::{FocusedElement, error_codes};
use synapse_perception::ObservationInput;

use super::action_preflight::ActionPreflightReadback;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EverQuestUiContextReadback {
    pub status: String,
    pub in_world: bool,
    pub login_screen_visible: bool,
    pub login_signal_names: Vec<String>,
    pub in_world_signal_names: Vec<String>,
    pub focused_text_role: Option<String>,
    pub focused_text_name: Option<String>,
    pub focused_text_value_len: Option<usize>,
    pub focused_text_selected_len: Option<usize>,
    pub source_mode: String,
}

pub(super) fn everquest_ui_context_from_input(
    input: &ObservationInput,
) -> EverQuestUiContextReadback {
    let login_signal_names = login_signal_names(input);
    let in_world_signal_names = in_world_signal_names(input);
    let focused = focused_text_summary(input.focused.as_ref());
    let login_screen_visible = !login_signal_names.is_empty();
    let in_world = !login_screen_visible && !in_world_signal_names.is_empty();
    let status = if login_screen_visible {
        "login_screen"
    } else if in_world {
        "in_world_ui"
    } else {
        "ambiguous_ui"
    };

    EverQuestUiContextReadback {
        status: status.to_owned(),
        in_world,
        login_screen_visible,
        login_signal_names,
        in_world_signal_names,
        focused_text_role: focused.as_ref().map(|summary| summary.role.clone()),
        focused_text_name: focused.as_ref().map(|summary| summary.name.clone()),
        focused_text_value_len: focused.as_ref().map(|summary| summary.value_len),
        focused_text_selected_len: focused.as_ref().map(|summary| summary.selected_len),
        source_mode: "everquest_hud_login_and_inventory_signal_scan".to_owned(),
    }
}

pub(super) fn deny_login_screen_action(
    tool: &'static str,
    preflight: &ActionPreflightReadback,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} denied because the visible EverQuest UI is a login/account gate, not in-world gameplay"
        ),
        Some(json!({
            "code": error_codes::SAFETY_PROFILE_ACTION_DENIED,
            "reason": "everquest_login_or_account_gate_visible",
            "action_preflight": preflight,
        })),
    )
}

fn login_signal_names(input: &ObservationInput) -> Vec<String> {
    let Some(reading) = input.hud.by_name.get("everquest.login_screen_text") else {
        return Vec::new();
    };
    let raw = reading.raw_text.to_ascii_lowercase();
    let mut signals = Vec::new();
    if raw.contains("username") {
        signals.push("username_label".to_owned());
    }
    if raw.contains("password") {
        signals.push("password_label".to_owned());
    }
    if raw.contains("quick connect") || raw.contains("quickconnect") {
        signals.push("quick_connect_button".to_owned());
    }
    if raw.contains("login") || raw.contains("log in") {
        signals.push("login_button".to_owned());
    }
    if raw.contains("eula") || raw.contains("end user license agreement") {
        signals.push("eula_agreement".to_owned());
    }
    if raw.contains("terms of service") || raw.contains("privacy policy") {
        signals.push("terms_or_privacy_policy".to_owned());
    }
    if raw.contains("i agree") {
        signals.push("agree_button".to_owned());
    }
    if raw.contains("i decline") {
        signals.push("decline_button".to_owned());
    }
    if signals.is_empty() {
        signals.push("login_hud_match".to_owned());
    }
    signals.sort();
    signals.dedup();
    signals
}

fn in_world_signal_names(input: &ObservationInput) -> Vec<String> {
    let mut signals = Vec::new();
    if input.hud.by_name.contains_key("everquest.level_text") {
        signals.push("inventory_level".to_owned());
    }
    if input.hud.by_name.contains_key("everquest.next_level_label") {
        signals.push("inventory_next_level_label".to_owned());
    }
    if input
        .hud
        .by_name
        .contains_key("everquest.next_level_percent")
    {
        signals.push("inventory_next_level_percent".to_owned());
    }
    if input.hud.by_name.contains_key("everquest.map_window_text") {
        signals.push("map_window".to_owned());
    }
    signals
}

#[derive(Clone, Debug)]
struct FocusedTextSummary {
    role: String,
    name: String,
    value_len: usize,
    selected_len: usize,
}

fn focused_text_summary(focused: Option<&FocusedElement>) -> Option<FocusedTextSummary> {
    let focused = focused?;
    let role = focused.role.to_ascii_lowercase();
    let name = focused.name.to_ascii_lowercase();
    let is_text_entry = role.contains("edit")
        || role.contains("text")
        || role.contains("document")
        || name.contains("username")
        || name.contains("password")
        || name.contains("login")
        || focused.patterns.iter().any(|pattern| {
            matches!(
                pattern,
                synapse_core::UiaPattern::Text | synapse_core::UiaPattern::Value
            )
        });
    is_text_entry.then(|| FocusedTextSummary {
        role: focused.role.clone(),
        name: focused.name.clone(),
        value_len: focused.value.as_deref().map_or("", str::trim).len(),
        selected_len: focused.selected_text.as_deref().map_or("", str::trim).len(),
    })
}
