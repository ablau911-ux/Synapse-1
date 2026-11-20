//! Frozen behaviour table (manager-owned, work method v3): other extensions
//! with debugger/nativeMessaging permission are diagnostic, never a reason to
//! disable them; commands fail closed only when the observation is unreadable.
//! Codex and Opus never edit this file. One case per L row.

use serde_json::{Value, json};

use super::external_popup_risk_allows_commands;

fn observed(remaining: u64) -> Value {
    json!({
        "ok": remaining == 0,
        "status": if remaining == 0 { "clear" } else { "observed" },
        "management_available": true,
        "hazard_count": remaining,
        "remaining_hazard_count": remaining,
        "disabled_count": 0,
        "failure_count": 0,
    })
}

#[test]
fn external_risk_gate_table_interop() {
    // (row, profile risk count, live observation, commands allowed)
    let rows: Vec<(&str, usize, Option<Value>, bool)> = vec![
        ("i5a_no_external_risk_allows", 0, None, true),
        (
            "i5b_observed_enabled_external_extension_allows",
            1,
            Some(observed(1)),
            true,
        ),
        (
            "i5b_boundary_many_observed_external_extensions_allow",
            3,
            Some(observed(3)),
            true,
        ),
        ("i5c_no_live_observation_refuses", 1, None, false),
        (
            "i5d_management_unavailable_refuses",
            1,
            Some(json!({
                "ok": false,
                "status": "management_unavailable",
                "management_available": false,
                "remaining_hazard_count": 0,
                "failure_count": 1,
            })),
            false,
        ),
        (
            "i5e_scan_error_refuses",
            1,
            Some(json!({
                "ok": false,
                "status": "suppression_error",
                "management_available": true,
                "remaining_hazard_count": 0,
                "failure_count": 1,
            })),
            false,
        ),
        (
            "i5f_hostile_observation_not_an_object_refuses",
            1,
            Some(json!("observed")),
            false,
        ),
        (
            "i5f_hostile_observation_wrong_types_refuses",
            1,
            Some(json!({ "management_available": "true", "failure_count": "0" })),
            false,
        ),
        (
            "i5f_hostile_observation_missing_failure_count_refuses",
            1,
            Some(json!({ "management_available": true })),
            false,
        ),
        (
            "i5g_neighbour_legacy_suppressed_observation_still_allows",
            2,
            Some(json!({
                "ok": true,
                "status": "suppressed",
                "management_available": true,
                "remaining_hazard_count": 0,
                "disabled_count": 2,
                "failure_count": 0,
            })),
            true,
        ),
    ];
    let mut failures = Vec::new();
    for (row, risk_count, observation, expected) in rows {
        let got = external_popup_risk_allows_commands(risk_count, observation.as_ref());
        if got != expected {
            failures.push(format!("row {row}: allowed {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "external risk gate table rows failed:\n{}",
        failures.join("\n")
    );
}
