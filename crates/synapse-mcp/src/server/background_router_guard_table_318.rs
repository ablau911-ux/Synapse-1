//! #318 frozen behaviour table (manager-owned, work method v3). Codex and the
//! review agent never edit this file. One case per L row of the table in Gitea
//! issue #318. The foreground outcome rule itself is pinned by the #300 and #316
//! tables and is reused, not copied.

use super::{background_guard_applies, foreground_read_blocks_background_action};
use synapse_core::error_codes::{A11Y_NO_FOREGROUND, A11Y_UIA_WORKER_TIMEOUT, OBSERVE_INTERNAL};

#[test]
fn background_guard_applies_table_318() {
    let rows: [(&str, Option<bool>, bool); 3] = [
        ("318f_background_reply_is_guarded", Some(false), true),
        (
            "318g_foreground_tier_reply_is_not_guarded",
            Some(true),
            false,
        ),
        ("318h_failed_action_is_still_guarded", None, true),
    ];
    let mut failures = Vec::new();
    for (row, reply, expected) in rows {
        let got = background_guard_applies(reply);
        if got != expected {
            failures.push(format!("row {row}: {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#318 table rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn foreground_read_blocks_background_action_table_318() {
    let rows: [(&str, &str, bool); 5] = [
        (
            "318i_no_foreground_window_does_not_block",
            A11Y_NO_FOREGROUND,
            false,
        ),
        ("318j_worker_timeout_blocks", A11Y_UIA_WORKER_TIMEOUT, true),
        ("318j_internal_error_blocks", OBSERVE_INTERNAL, true),
        ("318j_empty_code_blocks", "", true),
        (
            "318j_hostile_lookalike_code_blocks",
            "A11Y_NO_FOREGROUND ",
            true,
        ),
    ];
    let mut failures = Vec::new();
    for (row, code, expected) in rows {
        let got = foreground_read_blocks_background_action(code);
        if got != expected {
            failures.push(format!("row {row}: {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#318 table rows failed:\n{}",
        failures.join("\n")
    );
}
