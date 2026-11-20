//! Pure evaluation of the `ExtensionSettings` policy self-shield (moved out of
//! the chrome bridge): does the row's policy value block `nativeMessaging` for
//! the bridge extension with Synapse's marker message.

use serde_json::Value;

/// Permission the self-shield must block.
pub const SHIELDED_PERMISSION: &str = "nativeMessaging";

/// Result of reading one policy value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyShieldEvaluation {
    pub present: bool,
    /// `value_missing_or_unreadable`, `value_empty`, `parse_error`,
    /// `self_entry_missing`, `native_messaging_self_shield_active` or
    /// `self_entry_incomplete`.
    pub reason: &'static str,
    pub blocked_permissions: Vec<String>,
    pub marker_matches: bool,
    pub raw_len: usize,
}

impl PolicyShieldEvaluation {
    const fn absent(reason: &'static str, raw_len: usize) -> Self {
        Self {
            present: false,
            reason,
            blocked_permissions: Vec::new(),
            marker_matches: false,
            raw_len,
        }
    }
}

/// Evaluates the raw `ExtensionSettings` JSON for `extension_id`.
#[must_use]
pub fn evaluate_self_policy_shield(
    raw: Option<&str>,
    extension_id: &str,
    blocked_install_message: &str,
) -> PolicyShieldEvaluation {
    let Some(raw) = raw else {
        return PolicyShieldEvaluation::absent("value_missing_or_unreadable", 0);
    };
    if raw.trim().is_empty() {
        return PolicyShieldEvaluation::absent("value_empty", raw.len());
    }
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return PolicyShieldEvaluation::absent("parse_error", raw.len());
    };
    let Some(entry) = parsed.get(extension_id) else {
        return PolicyShieldEvaluation::absent("self_entry_missing", raw.len());
    };
    let blocked_permissions = entry
        .get("blocked_permissions")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let blocks_native_messaging = blocked_permissions
        .iter()
        .any(|permission| permission == SHIELDED_PERMISSION);
    let marker_matches = entry.get("blocked_install_message").and_then(Value::as_str)
        == Some(blocked_install_message);
    let present = blocks_native_messaging && marker_matches;
    PolicyShieldEvaluation {
        present,
        reason: if present {
            "native_messaging_self_shield_active"
        } else {
            "self_entry_incomplete"
        },
        blocked_permissions,
        marker_matches,
        raw_len: raw.len(),
    }
}
