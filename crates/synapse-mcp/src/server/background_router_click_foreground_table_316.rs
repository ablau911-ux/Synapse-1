//! #316 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the table in Gitea issue #316.

use super::{NativeForegroundOutcome, background_click_foreground_error};
use synapse_core::error_codes::ACTION_FOREGROUND_LOST;

const DECOY: i64 = 0x0D_0D0D;
const ELSEWHERE: i64 = 0x0E_0E0E;

#[test]
fn background_click_foreground_table_316() {
    // (row, outcome, restore result, expected error code; None = the click reply stands)
    let rows: [(&str, NativeForegroundOutcome, Option<bool>, Option<&str>); 7] = [
        (
            "316a_unchanged_is_ok",
            NativeForegroundOutcome::Unchanged,
            None,
            None,
        ),
        (
            "316b_taken_by_target_and_restored_is_ok",
            NativeForegroundOutcome::TakenByTarget {
                restore_to: Some(DECOY),
            },
            Some(true),
            None,
        ),
        (
            "316c_taken_by_target_restore_failed_is_foreground_lost",
            NativeForegroundOutcome::TakenByTarget {
                restore_to: Some(DECOY),
            },
            Some(false),
            Some(ACTION_FOREGROUND_LOST),
        ),
        (
            "316c_hostile_taken_by_target_restore_never_attempted_is_foreground_lost",
            NativeForegroundOutcome::TakenByTarget {
                restore_to: Some(DECOY),
            },
            None,
            Some(ACTION_FOREGROUND_LOST),
        ),
        (
            "316d_taken_by_target_with_no_prior_window_is_ok",
            NativeForegroundOutcome::TakenByTarget { restore_to: None },
            None,
            None,
        ),
        (
            "316e_moved_elsewhere_by_someone_else_is_ok_never_restored",
            NativeForegroundOutcome::MovedElsewhere {
                now: Some(ELSEWHERE),
            },
            None,
            None,
        ),
        (
            "316e_moved_to_no_window_is_ok",
            NativeForegroundOutcome::MovedElsewhere { now: None },
            None,
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (row, outcome, restored, expected) in rows {
        let got = background_click_foreground_error(outcome, restored);
        if got != expected {
            failures.push(format!("row {row}: error {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#316 table rows failed:\n{}",
        failures.join("\n")
    );
}
