//! Routing decision for `act operation=foreground` input verbs (#162).
//!
//! `act operation=foreground` is the audited real-OS-foreground lane. When the
//! claimed target is not the OS foreground, delegating a click/type/press/scroll
//! straight to `target_act` runs the background (non-foreground) preflight, which
//! refuses background WebView2 targets with `A11Y_CDP_UNREACHABLE` before any
//! input is sent. The foreground lane must instead focus the claimed target with
//! the verified `act_focus_window` step and then deliver through the foreground
//! tier. The background preflight stays on `operation=invoke`.

/// Verbs that deliver real input to the claimed target and therefore need the
/// claimed window to be the OS foreground in the foreground lane.
pub(crate) const FOREGROUND_INPUT_VERBS: &[&str] = &[
    "click",
    "dblclick",
    "type",
    "key",
    "key_chord",
    "press",
    "scroll",
];

/// True when the foreground lane must verified-focus the claimed target before
/// delegating the action.
pub(crate) fn act_foreground_requires_claimed_focus(
    foreground_operation: bool,
    verb: &str,
    has_key_chord: bool,
) -> bool {
    foreground_operation && (has_key_chord || FOREGROUND_INPUT_VERBS.contains(&verb))
}

#[cfg(test)]
#[path = "tests/act_foreground_focus_162.rs"]
mod tests;
