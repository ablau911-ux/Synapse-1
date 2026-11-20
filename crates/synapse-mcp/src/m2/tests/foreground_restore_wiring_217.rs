//! #217: the lane's recorded own focus must reach the real restore step.
//!
//! The existing #217 tests call `foreground_restore_decision` with a hand-built
//! `OwnFocus`. These tests go through the snapshot instead: the own focus is
//! recorded with `record_own_focus_with` (the production recorder) and the
//! restore runs through the snapshot's restore step, so dropping the recorded
//! focus anywhere between the two fails them.
//!
//! NEEDS-SEAM (G5b raid): the part of `ForegroundInputContextSnapshot::
//! restore_foreground` after the prior window was proven alive and the current
//! foreground was read must be reachable as
//! `fn restore_foreground_for_current(&self, tool: &'static str, session_id: &str,
//!  current: &foreground_own_focus::CurrentForeground,
//!  owner_chain: impl FnOnce(i64) -> Vec<i64>,
//!  restore_own: impl FnOnce() -> &'static str) -> &'static str`
//! returning "already_current", the `restore_own` outcome, or
//! "skipped_human_moved". Production passes `live_owner_chain` and
//! `|| self.restore_own_focus(..)`. A real foreground change on the owner's
//! desktop is never made by these tests.

use std::cell::Cell;

use super::ForegroundInputContextSnapshot;
use super::foreground_own_focus::CurrentForeground;

const TOOL: &str = "act_foreground";
const SESSION: &str = "issue217-restore-wiring";
const PRIOR: (i64, u32) = (0x1111, 100);
const TARGET: (i64, u32) = (0x2222, 200);

fn snapshot() -> ForegroundInputContextSnapshot {
    ForegroundInputContextSnapshot {
        cursor: None,
        foreground_hwnd: Some(PRIOR.0),
        foreground_pid: Some(PRIOR.1),
        foreground_process_started_at_100ns: Some(1),
        foreground_title: Some("human editor".to_owned()),
        cursor_capture_error: None,
        foreground_capture_error: None,
        own_focus: None,
    }
}

fn record(snapshot: &mut ForegroundInputContextSnapshot, read: Result<(i64, u32), String>) {
    snapshot.record_own_focus_with(TOOL, SESSION, || read);
}

/// Runs the restore step; returns the outcome and how often the prior window
/// was actually restored.
fn restore(
    snapshot: &ForegroundInputContextSnapshot,
    current: (i64, u32),
    owner_chain: Vec<i64>,
) -> (&'static str, u32) {
    let restores = Cell::new(0_u32);
    let current = CurrentForeground {
        hwnd: current.0,
        pid: current.1,
        title: "current".to_owned(),
    };
    let outcome = snapshot.restore_foreground_for_current(
        TOOL,
        SESSION,
        &current,
        |_| owner_chain,
        || {
            restores.set(restores.get() + 1);
            "restored"
        },
    );
    (outcome, restores.get())
}

#[test]
fn must_pass_recorded_own_focus_on_the_target_restores_the_prior_window() {
    let mut snapshot = snapshot();
    record(&mut snapshot, Ok(TARGET));
    assert_eq!(snapshot.own_focus.map(|own| own.hwnd), Some(TARGET.0));

    assert_eq!(restore(&snapshot, TARGET, Vec::new()), ("restored", 1));
}

#[test]
fn must_pass_a_modal_owned_by_the_focused_target_restores_the_prior_window() {
    let mut snapshot = snapshot();
    record(&mut snapshot, Ok(TARGET));

    assert_eq!(
        restore(&snapshot, (0x3333, TARGET.1), vec![TARGET.0]),
        ("restored", 1)
    );
}

#[test]
fn must_fail_another_window_of_the_same_process_is_a_human_move() {
    let mut snapshot = snapshot();
    record(&mut snapshot, Ok(TARGET));

    assert_eq!(
        restore(&snapshot, (0x4444, TARGET.1), Vec::new()),
        ("skipped_human_moved", 0)
    );
}

#[test]
fn must_fail_a_failed_own_focus_read_leaves_a_target_foreground_as_human_move() {
    let mut snapshot = snapshot();
    record(
        &mut snapshot,
        Err("GetForegroundWindow returned null".to_owned()),
    );
    assert!(
        snapshot.own_focus.is_none(),
        "precondition: nothing recorded"
    );

    assert_eq!(
        restore(&snapshot, TARGET, Vec::new()),
        ("skipped_human_moved", 0)
    );
}

#[test]
fn must_pass_prior_window_still_foreground_needs_no_restore() {
    let mut snapshot = snapshot();
    record(&mut snapshot, Ok(TARGET));

    assert_eq!(
        restore(&snapshot, PRIOR, Vec::new()),
        ("already_current", 0)
    );
}

const M2_SOURCE: &str = include_str!("../../m2.rs");
const ROUTER_SOURCE: &str = include_str!("../../server/background_router.rs");

fn body_after<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} not found"));
    let rest = &source[start..];
    let end = ["\n    }\n", "\n    }\r\n"]
        .iter()
        .filter_map(|closing| rest.find(closing))
        .min()
        .unwrap_or(rest.len());
    &rest[..end]
}

/// SOURCE-SCAN GUARD: the live restore uses the seam with the live owner chain
/// and the real own-focus restore, and the lane records its own focus after the
/// verified focus step and before the action runs.
#[test]
fn must_fail_live_wiring_never_drops_the_recorded_own_focus() {
    let restore = body_after(M2_SOURCE, "fn restore_foreground(");
    for needle in [
        "restore_foreground_for_current(",
        "live_owner_chain",
        "restore_own_focus(",
    ] {
        assert!(
            restore.contains(needle),
            "restore_foreground must use {needle}"
        );
    }
    let lane = body_after(ROUTER_SOURCE, "async fn act_foreground_focused_action(");
    let focus = lane
        .find(".act_foreground_focus_key_target(")
        .expect("focus step");
    let record = lane
        .find("guard.record_own_foreground_focus();")
        .expect("own-focus record");
    let action = lane.find("self.target_act(").expect("action step");
    assert!(
        focus < record && record < action,
        "record own focus between focus and action"
    );
}
