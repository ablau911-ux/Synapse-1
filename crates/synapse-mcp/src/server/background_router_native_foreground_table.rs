//! Frozen behaviour table for issue #300 row 300g: after a native UIA pattern action
//! (Toggle / Invoke) on a background target, one pure decision says what happened to the
//! OS foreground and whether Synapse must put the prior window back.
//!
//! Written by the manager from the behaviour table in issue #300 (work method v3), before
//! the code. Code agents must not edit this file.
//!
//! Inputs are ROOT (top-level) window handles: the foreground root before the action, the
//! foreground root after it, and the session target's root. `None` = no foreground window.

use super::{NativeForegroundOutcome, native_pattern_foreground_outcome};

const OWNER: i64 = 0x00b1_0044;
const TARGET: i64 = 0x0163_048e;
const OTHER: i64 = 0x00a8_0658;

#[test]
fn native_foreground_outcome_table() {
    // normal: the foreground never moved -> nothing to restore
    assert_eq!(
        native_pattern_foreground_outcome(Some(OWNER), Some(OWNER), TARGET),
        NativeForegroundOutcome::Unchanged,
        "row unchanged"
    );
    // the app's own UIA provider pulled the target forward -> restore the owner's window
    assert_eq!(
        native_pattern_foreground_outcome(Some(OWNER), Some(TARGET), TARGET),
        NativeForegroundOutcome::TakenByTarget {
            restore_to: Some(OWNER)
        },
        "row taken_by_target"
    );
    // the foreground moved to a third window (the human clicked) -> never restore
    assert_eq!(
        native_pattern_foreground_outcome(Some(OWNER), Some(OTHER), TARGET),
        NativeForegroundOutcome::MovedElsewhere { now: Some(OTHER) },
        "row human_moved_never_restore"
    );
    // the target was already in front -> unchanged, nothing to restore
    assert_eq!(
        native_pattern_foreground_outcome(Some(TARGET), Some(TARGET), TARGET),
        NativeForegroundOutcome::Unchanged,
        "row target_already_front"
    );
    // empty: no foreground before (locked / desktop), target took it -> nothing to restore to
    assert_eq!(
        native_pattern_foreground_outcome(None, Some(TARGET), TARGET),
        NativeForegroundOutcome::TakenByTarget { restore_to: None },
        "row no_prior_foreground"
    );
    // foreground vanished after the action (window closed / lock) -> report, never restore
    assert_eq!(
        native_pattern_foreground_outcome(Some(OWNER), None, TARGET),
        NativeForegroundOutcome::MovedElsewhere { now: None },
        "row foreground_gone"
    );
    // only TakenByTarget with a real prior window asks for a restore
    assert_eq!(
        NativeForegroundOutcome::TakenByTarget {
            restore_to: Some(OWNER)
        }
        .restore_target(),
        Some(OWNER),
        "row restore_target_some"
    );
    assert_eq!(
        NativeForegroundOutcome::Unchanged.restore_target(),
        None,
        "row unchanged_no_restore"
    );
    assert_eq!(
        NativeForegroundOutcome::MovedElsewhere { now: Some(OTHER) }.restore_target(),
        None,
        "row moved_no_restore"
    );
    // the reply may claim "foreground untouched" only for Unchanged
    assert!(
        NativeForegroundOutcome::Unchanged.foreground_untouched(),
        "row claim_only_unchanged"
    );
    assert!(
        !NativeForegroundOutcome::TakenByTarget {
            restore_to: Some(OWNER)
        }
        .foreground_untouched(),
        "row no_claim_when_taken"
    );
    assert!(
        !NativeForegroundOutcome::MovedElsewhere { now: Some(OTHER) }.foreground_untouched(),
        "row no_claim_when_moved"
    );
}
