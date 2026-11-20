//! #217: after `act operation=foreground` the lane's own claimed-focus step must
//! not be mistaken for a human foreground move, so the human's window is restored.
//! Only the focused window itself, or a window it owns (a modal/dialog the target
//! opened), counts as own focus; another window of the same process is a human move.

use super::{
    ForegroundRestoreCurrentDecision,
    foreground_own_focus::{OwnFocus, foreground_restore_decision},
};

const PRIOR_HWND: i64 = 0x1001; // human's VS Code, captured before the lane acted
const PRIOR_PID: u32 = 42;
const TARGET_HWND: i64 = 0x2002; // claimed target the lane itself focused
const TARGET_PID: u32 = 77;
const HUMAN_HWND: i64 = 0x3003; // window the human switched to mid-action
const HUMAN_PID: u32 = 99;
const TARGET_MODAL_HWND: i64 = 0x2003; // dialog the target opened after the lane's input
const SAME_PROCESS_OTHER_HWND: i64 = 0x2004; // e.g. another Chrome window, same pid, unowned
const OTHER_OWNER_HWND: i64 = 0x2005; // a different top-level window of the same process
const TARGET_FOCUS: OwnFocus = OwnFocus {
    hwnd: TARGET_HWND,
    pid: TARGET_PID,
};

fn no_owner(_hwnd: i64) -> Vec<i64> {
    Vec::new()
}

#[test]
fn must_pass_a_modal_the_target_opened_is_still_the_lanes_own_focus() {
    let decision = foreground_restore_decision(
        PRIOR_HWND,
        PRIOR_PID,
        TARGET_MODAL_HWND,
        TARGET_PID,
        Some(TARGET_FOCUS),
        |hwnd| {
            assert_eq!(
                hwnd, TARGET_MODAL_HWND,
                "owner chain is read for the current foreground"
            );
            vec![TARGET_HWND]
        },
    );
    assert_eq!(decision, ForegroundRestoreCurrentDecision::RestoreOwnFocus);
}

#[test]
fn must_pass_a_modal_owned_by_the_focused_target_is_own_focus() {
    // Nested dialog: modal -> intermediate owned window -> focused target.
    let decision = foreground_restore_decision(
        PRIOR_HWND,
        PRIOR_PID,
        TARGET_MODAL_HWND,
        TARGET_PID,
        Some(TARGET_FOCUS),
        |_| vec![OTHER_OWNER_HWND, TARGET_HWND],
    );
    assert_eq!(decision, ForegroundRestoreCurrentDecision::RestoreOwnFocus);
}

#[test]
fn must_pass_lane_own_focus_is_restored_not_skipped_as_human_move() {
    let decision = foreground_restore_decision(
        PRIOR_HWND,
        PRIOR_PID,
        TARGET_HWND,
        TARGET_PID,
        Some(TARGET_FOCUS),
        no_owner,
    );
    assert_eq!(decision, ForegroundRestoreCurrentDecision::RestoreOwnFocus);
}

#[test]
fn must_fail_another_window_of_the_same_process_is_a_human_move() {
    // Human switched from Chrome window B (lane focus) to Chrome window C.
    let decision = foreground_restore_decision(
        PRIOR_HWND,
        PRIOR_PID,
        SAME_PROCESS_OTHER_HWND,
        TARGET_PID,
        Some(TARGET_FOCUS),
        no_owner,
    );
    assert_eq!(decision, ForegroundRestoreCurrentDecision::SkipHumanMoved);
}

#[test]
fn must_fail_a_window_owned_by_a_different_window_is_a_human_move() {
    let decision = foreground_restore_decision(
        PRIOR_HWND,
        PRIOR_PID,
        TARGET_MODAL_HWND,
        TARGET_PID,
        Some(TARGET_FOCUS),
        |_| vec![OTHER_OWNER_HWND],
    );
    assert_eq!(decision, ForegroundRestoreCurrentDecision::SkipHumanMoved);
}

#[test]
fn must_fail_foreground_moved_without_recorded_own_focus_is_human_move() {
    let decision =
        foreground_restore_decision(PRIOR_HWND, PRIOR_PID, TARGET_HWND, TARGET_PID, None, |_| {
            vec![TARGET_HWND]
        });
    assert_eq!(decision, ForegroundRestoreCurrentDecision::SkipHumanMoved);
}

#[test]
fn must_fail_human_switching_after_lane_focus_is_still_human_move() {
    let decision = foreground_restore_decision(
        PRIOR_HWND,
        PRIOR_PID,
        HUMAN_HWND,
        HUMAN_PID,
        Some(TARGET_FOCUS),
        no_owner,
    );
    assert_eq!(decision, ForegroundRestoreCurrentDecision::SkipHumanMoved);
}

#[test]
fn must_pass_prior_window_already_foreground_needs_no_restore() {
    let decision = foreground_restore_decision(
        PRIOR_HWND,
        PRIOR_PID,
        PRIOR_HWND,
        PRIOR_PID,
        Some(TARGET_FOCUS),
        no_owner,
    );
    assert_eq!(decision, ForegroundRestoreCurrentDecision::AlreadyCurrent);
}
