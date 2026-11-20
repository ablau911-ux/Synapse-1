//! Frozen behaviour table for issue #313 row 313a: one config knob decides whether this
//! daemon instance may show Windows toasts on the desktop.
//!
//! Written by the manager from the behaviour table in issue #313 (work method v3), before
//! the code. Code agents must not edit this file.
//!
//! Knob: environment variable `SYNAPSE_DESKTOP_TOASTS`. The pure decision receives its raw
//! value (`None` when unset) and returns the suppression reason, or `None` when toasts are
//! allowed. The reason is the text recorded in escalation rows (`tier0_suppressed_reason`)
//! and in toast replies.

use super::{DESKTOP_TOASTS_DISABLED_REASON, desktop_toasts_suppressed_reason};

#[test]
fn desktop_toasts_knob_table() {
    // normal / live default: knob unset -> toasts allowed
    assert_eq!(
        desktop_toasts_suppressed_reason(None),
        None,
        "row unset_allows"
    );
    // explicit on
    assert_eq!(
        desktop_toasts_suppressed_reason(Some("on")),
        None,
        "row on_allows"
    );
    // off silences this instance, with the one fixed reason
    assert_eq!(
        desktop_toasts_suppressed_reason(Some("off")),
        Some(DESKTOP_TOASTS_DISABLED_REASON),
        "row off_suppresses"
    );
    // case and surrounding blanks do not matter
    assert_eq!(
        desktop_toasts_suppressed_reason(Some("  OFF ")),
        Some(DESKTOP_TOASTS_DISABLED_REASON),
        "row off_case_and_blanks"
    );
    // the usual false spellings also silence
    for value in ["false", "0", "no"] {
        assert_eq!(
            desktop_toasts_suppressed_reason(Some(value)),
            Some(DESKTOP_TOASTS_DISABLED_REASON),
            "row false_spellings {value}"
        );
    }
    // empty value = unset: toasts allowed (the live owner must keep his alerts)
    assert_eq!(
        desktop_toasts_suppressed_reason(Some("")),
        None,
        "row empty_allows"
    );
    // unknown / hostile value: toasts stay allowed, never a silent loss of owner alerts
    assert_eq!(
        desktop_toasts_suppressed_reason(Some("maybe")),
        None,
        "row unknown_allows"
    );
    assert_eq!(
        desktop_toasts_suppressed_reason(Some("off\u{0}; rm -rf")),
        None,
        "row hostile_allows"
    );
    // the reason names the knob so an operator can find it
    assert!(
        DESKTOP_TOASTS_DISABLED_REASON.contains("SYNAPSE_DESKTOP_TOASTS"),
        "row reason_names_knob"
    );
}
