//! #315 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. Row 315f: the action layer keeps "the window is gone"
//! distinct from "no foreground" and from "element not resolved".

use super::*;
use synapse_core::error_codes::{
    ACTION_ELEMENT_NOT_RESOLVED, TARGET_WINDOW_NOT_FOUND, TRANSIENT_ELEMENT_EXPIRED,
};

#[test]
fn window_gone_mapping_table_315() {
    let id = synapse_core::element_id(315, "01020304");
    let mut failures = Vec::new();

    match synapse_a11y::hwnd_validity(0x5a_041a, false) {
        Ok(()) => failures.push("row 315f: closed window reported usable".to_owned()),
        Err(error) => {
            let code = a11y_error_to_action(&id, error).code();
            if code != TARGET_WINDOW_NOT_FOUND {
                failures.push(format!(
                    "row 315f_closed_window_maps_to_target_window_not_found: code {code}, expected {TARGET_WINDOW_NOT_FOUND}"
                ));
            }
        }
    }

    // Neighbours that must stay as they are today.
    let unchanged = [
        (
            "315f_no_foreground_unchanged",
            synapse_a11y::A11yError::NoForeground {
                detail: "HWND was null".to_owned(),
            },
            ACTION_ELEMENT_NOT_RESOLVED,
        ),
        (
            "315f_stale_element_unchanged",
            synapse_a11y::A11yError::ElementStale {
                detail: "element not available".to_owned(),
            },
            TRANSIENT_ELEMENT_EXPIRED,
        ),
    ];
    for (row, error, expected) in unchanged {
        let code = a11y_error_to_action(&id, error).code();
        if code != expected {
            failures.push(format!("row {row}: code {code}, expected {expected}"));
        }
    }

    assert!(
        failures.is_empty(),
        "#315 table rows failed:\n{}",
        failures.join("\n")
    );
}
