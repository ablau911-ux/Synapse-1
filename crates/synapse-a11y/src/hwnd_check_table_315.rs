//! #315 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the table in Gitea issue #315.

use super::{as_foreground_error, hwnd_validity};
use crate::A11yError;
use synapse_core::error_codes::{
    A11Y_ELEMENT_STALE, A11Y_NO_FOREGROUND, OBSERVE_INTERNAL, TARGET_WINDOW_NOT_FOUND,
};

#[test]
fn hwnd_validity_table_315() {
    // (row, raw HWND, IsWindow answer, expected error code or None = usable)
    let rows: [(&str, i64, bool, Option<&str>); 6] = [
        (
            "315a_null_is_no_foreground",
            0,
            false,
            Some(A11Y_NO_FOREGROUND),
        ),
        (
            "315a_null_even_if_is_window_says_true",
            0,
            true,
            Some(A11Y_NO_FOREGROUND),
        ),
        (
            "315b_closed_window_is_target_window_not_found",
            0x5a_041a,
            false,
            Some(TARGET_WINDOW_NOT_FOUND),
        ),
        (
            "315b_hostile_negative_handle_is_target_window_not_found",
            -1,
            false,
            Some(TARGET_WINDOW_NOT_FOUND),
        ),
        (
            "315b_hostile_min_handle_is_target_window_not_found",
            i64::MIN,
            false,
            Some(TARGET_WINDOW_NOT_FOUND),
        ),
        ("315c_live_window_is_usable", 0x5a_041a, true, None),
    ];
    let mut failures = Vec::new();
    for (row, raw, is_window, expected) in rows {
        let error = hwnd_validity(raw, is_window).err();
        let code = error.as_ref().map(A11yError::code);
        if code != expected {
            failures.push(format!("row {row}: code {code:?}, expected {expected:?}"));
            continue;
        }
        if expected == Some(TARGET_WINDOW_NOT_FOUND)
            && let Some(error) = error
        {
            let message = error.to_string();
            if message.to_ascii_lowercase().contains("foreground") {
                failures.push(format!(
                    "row {row}: message mentions the foreground: {message}"
                ));
            }
            if !message.contains(&format!("0x{raw:x}")) {
                failures.push(format!(
                    "row {row}: message does not name HWND 0x{raw:x}: {message}"
                ));
            }
        }
    }

    // 315d: an HWND read from the OS foreground that vanished before its own
    // readback is still "no foreground" (neighbour behaviour, unchanged).
    match hwnd_validity(0x5a_041a, false) {
        Ok(()) => failures.push("row 315d: closed window reported usable".to_owned()),
        Err(error) => {
            let code = as_foreground_error(error).code();
            if code != A11Y_NO_FOREGROUND {
                failures.push(format!("row 315d_vanished_foreground_is_no_foreground: code {code}, expected {A11Y_NO_FOREGROUND}"));
            }
        }
    }
    // 315e: every other error passes through the foreground mapping unchanged.
    let passthrough = [
        (
            "315e_stale_unchanged",
            A11yError::ElementStale {
                detail: "gone".to_owned(),
            },
            A11Y_ELEMENT_STALE,
        ),
        (
            "315e_internal_unchanged",
            A11yError::internal("boom"),
            OBSERVE_INTERNAL,
        ),
        (
            "315e_no_foreground_unchanged",
            A11yError::NoForeground {
                detail: "none".to_owned(),
            },
            A11Y_NO_FOREGROUND,
        ),
    ];
    for (row, error, expected) in passthrough {
        let code = as_foreground_error(error).code();
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
