//! Frozen behaviour table for #312 row 312a: a routine or reflex delivers OS input
//! only to its stored target window, and only while that window is claimed and in front.
//!
//! Written by the manager from the behaviour table in issue #312 (work method v3).
//! Code agents must not edit this file. Deviation logged in #312: written after the
//! code; red evidence is the old build (4590a818), which had no stored target at all
//! (every routine/reflex input was refused with no way to bind a window).

use synapse_core::types::StoredInputTarget;

use super::require_stored_input_target;

const A: StoredInputTarget = StoredInputTarget {
    root_hwnd: 0x1234,
    pid: 100,
};
const B: StoredInputTarget = StoredInputTarget {
    root_hwnd: 0x5678,
    pid: 200,
};
const A_OTHER_PID: StoredInputTarget = StoredInputTarget {
    root_hwnd: 0x1234,
    pid: 101,
};

#[test]
fn stored_target_table() {
    // row normal: stored A, A claimed and in front -> deliver to A
    let ok = require_stored_input_target("routine-1", Some(A), Some(A));
    assert!(ok.as_ref().is_ok_and(|t| *t == A), "row normal: {ok:?}");

    // row absent: no stored target, nothing in front -> required, names the id
    let absent = require_stored_input_target("routine-1", None, None);
    let e = absent.as_ref().err();
    assert!(
        e.is_some_and(|e| e.code() == "ACTION_INPUT_TARGET_REQUIRED"
            && e.to_string().contains("routine-1")
            && e.to_string()
                .contains("no stored target window; set one when creating or arming")),
        "row absent: {absent:?}"
    );

    // row absent_but_something_claimed: never falls back to the claimed window
    let fallback = require_stored_input_target("reflex-7", None, Some(B));
    assert!(
        fallback
            .as_ref()
            .err()
            .is_some_and(|e| e.code() == "ACTION_INPUT_TARGET_REQUIRED"
                && e.to_string().contains("reflex-7")),
        "row absent_but_something_claimed: {fallback:?}"
    );

    // row not_in_front: stored A, nothing claimed in front -> existing not-ready reason
    let none_front = require_stored_input_target("routine-1", Some(A), None);
    assert!(
        none_front
            .as_ref()
            .err()
            .is_some_and(|e| e.code() == "ACTION_INPUT_TARGET_NOT_READY"),
        "row not_in_front: {none_front:?}"
    );

    // row other_window_in_front: stored A, B claimed in front -> refused
    let other = require_stored_input_target("routine-1", Some(A), Some(B));
    assert!(
        other
            .as_ref()
            .err()
            .is_some_and(|e| e.code() == "ACTION_INPUT_TARGET_NOT_READY"),
        "row other_window_in_front: {other:?}"
    );

    // row reused_hwnd: same hwnd, different pid (window closed, handle reused) -> refused
    let reused = require_stored_input_target("routine-1", Some(A), Some(A_OTHER_PID));
    assert!(
        reused
            .as_ref()
            .err()
            .is_some_and(|e| e.code() == "ACTION_INPUT_TARGET_NOT_READY"),
        "row reused_hwnd: {reused:?}"
    );
}
