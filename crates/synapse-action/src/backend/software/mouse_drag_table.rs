//! Frozen behaviour table for #309 rows 309a and 309b: drag release and drag timing.
//!
//! Written by the manager from the behaviour table in issue #309 (work method v3).
//! Code agents must not edit this file. Deviation logged in #309: written after the
//! code; red evidence is the old build (4b003b82), whose early `?` skipped the
//! button-up on a failed move and sent every drag point at once.

use super::{drag_outcome, drag_sample_offset_ms};
use crate::ActionError;

fn err(detail: &str) -> ActionError {
    ActionError::BackendUnavailable {
        detail: detail.to_owned(),
    }
}

#[test]
fn drag_outcome_table() {
    // normal: move and release both fine
    assert!(drag_outcome(Ok(()), Ok(())).is_ok(), "row ok_ok");
    // move failed, release fine: the move error is returned
    let moved_failed = drag_outcome(Err(err("move-broke")), Ok(()));
    assert!(
        moved_failed
            .as_ref()
            .is_err_and(|e| e.to_string().contains("move-broke")),
        "row move_failed: {moved_failed:?}"
    );
    // move fine, release failed: the release error wins (the button may be held)
    let release_failed = drag_outcome(Ok(()), Err(err("up-broke")));
    assert!(
        release_failed
            .as_ref()
            .is_err_and(|e| e.to_string().contains("up-broke")),
        "row release_failed: {release_failed:?}"
    );
    // both failed: an error that names both
    let both = drag_outcome(Err(err("move-broke")), Err(err("up-broke")));
    let text = both
        .as_ref()
        .map_or_else(ToString::to_string, |()| String::new());
    assert!(
        text.contains("up-broke") && text.contains("move-broke"),
        "row both_failed: {text}"
    );
}

#[test]
fn drag_timing_table() {
    // (row, duration_ms, index, count, expected offset ms)
    let rows: [(&str, u32, usize, usize, u64); 6] = [
        ("no_samples", 600, 0, 0, 0),
        ("zero_duration", 0, 3, 5, 0),
        ("first_of_four", 600, 0, 4, 150),
        ("middle_of_four", 600, 1, 4, 300),
        ("last_lands_on_duration", 600, 3, 4, 600),
        ("index_past_end_clamps", 600, 9, 4, 600),
    ];
    for (name, duration, index, count, expected) in rows {
        assert_eq!(
            drag_sample_offset_ms(duration, index, count),
            expected,
            "row {name}"
        );
    }
}
