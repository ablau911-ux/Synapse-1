//! Whether other browser extensions with debugger/nativeMessaging permission
//! stop normal bridge commands (upstream #2265 browser interoperability).
//!
//! Other extensions belong to the owner. Synapse observes them and never
//! disables them; their presence is diagnostic. Commands are refused only
//! when the bridge could not read the extension list honestly.

use serde_json::Value;

/// Whether normal bridge commands may run, given how many external popup-risk
/// rows the profile scan found and the live bridge's management observation.
/// Allowed when there is no risk, or when the bridge read the extension list
/// (`management_available` and zero `failure_count`); never requires that the
/// owner's extensions were disabled.
pub(super) fn external_popup_risk_allows_commands(
    profile_risk_count: usize,
    observation: Option<&Value>,
) -> bool {
    if profile_risk_count == 0 {
        return true;
    }
    let Some(observation) = observation else {
        return false;
    };
    observation
        .get("management_available")
        .and_then(Value::as_bool)
        == Some(true)
        && observation.get("failure_count").and_then(Value::as_u64) == Some(0)
}

#[cfg(test)]
#[path = "chrome_bridge_external_risk_gate_table_interop.rs"]
mod table_interop;
