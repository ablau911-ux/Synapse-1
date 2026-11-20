//! #315 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. Row 315g: the perception layer reports a closed window
//! as TARGET_WINDOW_NOT_FOUND, not as "no perception available" or internal.

use super::*;
use synapse_core::error_codes::{
    A11Y_ELEMENT_STALE, OBSERVE_NO_PERCEPTION_AVAILABLE, TARGET_WINDOW_NOT_FOUND,
};

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn window_gone_mapping_table_315() {
    let mut failures = Vec::new();

    match synapse_a11y::hwnd_validity(0x5a_041a, false) {
        Ok(()) => failures.push("row 315g: closed window reported usable".to_owned()),
        Err(error) => {
            let mapped = a11y_error(&error);
            let code = error_code(&mapped);
            if code != Some(TARGET_WINDOW_NOT_FOUND) {
                failures.push(format!(
                    "row 315g_closed_window_maps_to_target_window_not_found: code {code:?}, expected {TARGET_WINDOW_NOT_FOUND}"
                ));
            }
        }
    }

    // Neighbours that must stay as they are today.
    let unchanged = [
        (
            "315g_no_foreground_unchanged",
            synapse_a11y::A11yError::NoForeground {
                detail: "HWND was null".to_owned(),
            },
            OBSERVE_NO_PERCEPTION_AVAILABLE,
        ),
        (
            "315g_stale_element_unchanged",
            synapse_a11y::A11yError::ElementStale {
                detail: "gone".to_owned(),
            },
            A11Y_ELEMENT_STALE,
        ),
    ];
    for (row, error, expected) in unchanged {
        let mapped = a11y_error(&error);
        let code = error_code(&mapped);
        if code != Some(expected) {
            failures.push(format!("row {row}: code {code:?}, expected {expected}"));
        }
    }

    assert!(
        failures.is_empty(),
        "#315 table rows failed:\n{}",
        failures.join("\n")
    );
}
