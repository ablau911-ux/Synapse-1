use std::collections::HashSet;

use rmcp::ErrorData;
use synapse_action::ActionError;
use synapse_core::{Key, KeyCode, error_codes};

use super::action_error_to_mcp;
use crate::m1::mcp_error;

pub(in crate::m2::press) const MODIFIER_ORDER: [&str; 4] = ["ctrl", "shift", "alt", "super"];

pub(in crate::m2::press) fn normalized_keys(raw_keys: &[String]) -> Result<Vec<Key>, ErrorData> {
    if raw_keys.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "act_press keys must contain at least one key",
        ));
    }

    let mut seen = HashSet::new();
    let mut names = Vec::with_capacity(raw_keys.len());
    for raw_key in raw_keys {
        let name = canonical_key_name(raw_key)?;
        if !seen.insert(name.clone()) {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                // Never echo a text-producing key: errors land in the audit log.
                if is_text_producing_key(&name) {
                    "act_press duplicate text-producing key".to_owned()
                } else {
                    format!("act_press duplicate key '{name}'")
                },
            ));
        }
        names.push(name);
    }

    let mut ordered = Vec::with_capacity(names.len());
    for modifier in MODIFIER_ORDER {
        if names.iter().any(|name| name == modifier) {
            ordered.push(key(modifier));
        }
    }
    for name in names
        .iter()
        .filter(|name| !MODIFIER_ORDER.contains(&name.as_str()))
    {
        ordered.push(key(name));
    }
    Ok(ordered)
}

pub(crate) fn canonical_key_name(raw_key: &str) -> Result<String, ErrorData> {
    let lowered = raw_key.trim().to_ascii_lowercase();
    let key = match lowered.as_str() {
        "" => {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "act_press key names must be non-empty",
            ));
        }
        "control" => "ctrl",
        "escape" => "esc",
        "return" => "enter",
        "backtick" | "grave" | "graveaccent" | "keyboardgraveaccent" => "`",
        "leftshift" | "lshift" | "rightshift" | "rshift" => "shift",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        "win" | "windows" | "meta" => "super",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        other => other,
    };

    if is_allowed_key_name(key) {
        Ok(key.to_owned())
    } else {
        Err(action_error_to_mcp(&ActionError::UnsupportedKey {
            // The raw name is not echoed: errors land in the audit log, and a
            // mistaken secret passed as a key name must not be persisted.
            detail: format!(
                "act_press unsupported key name ({} chars); use a single ASCII letter/digit or backtick, f1-f24, or a named key such as ctrl, shift, alt, super, enter, tab, esc, space, arrows, home/end, pageup/pagedown, insert, delete, backspace",
                raw_key.chars().count()
            ),
        }))
    }
}

fn is_allowed_key_name(key: &str) -> bool {
    if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() {
        return true;
    }
    if let Some(number) = key
        .strip_prefix('f')
        .and_then(|suffix| suffix.parse::<u8>().ok())
    {
        return (1..=24).contains(&number);
    }
    matches!(
        key,
        "`" | "alt"
            | "backspace"
            | "ctrl"
            | "delete"
            | "down"
            | "end"
            | "enter"
            | "esc"
            | "home"
            | "insert"
            | "left"
            | "pagedown"
            | "pageup"
            | "right"
            | "shift"
            | "space"
            | "super"
            | "tab"
            | "up"
    )
}

/// Whether a canonical key name inserts text when pressed without a shortcut
/// modifier: single printable characters and `space`.
pub(crate) fn is_text_producing_key(name: &str) -> bool {
    name == "space" || name.chars().count() == 1
}

pub(in crate::m2::press) fn key(value: &str) -> Key {
    Key {
        code: KeyCode::Named {
            value: value.to_owned(),
        },
        use_scancode: false,
    }
}
