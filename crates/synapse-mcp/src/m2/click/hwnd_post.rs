//! #107: Win32-faithful helpers for PostMessage mouse delivery to a resolved
//! child HWND: double-click message order and per-message target revalidation.
//!
//! The child HWND is resolved once and then held across press holds of up to
//! `MAX_CLICK_HOLD_MS` per click. HWND values are recycled by Windows, so every
//! posted message first re-reads the child identity (alive, still a descendant
//! of the claimed root, same owner pid, same lifetime generation) and fails
//! closed on any drift.

use synapse_action::ActionError;

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        CS_DBLCLKS, GCL_STYLE, GetClassLongW, GetWindowThreadProcessId, IsChild, IsWindow,
        WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_RBUTTONDBLCLK,
        WM_RBUTTONDOWN,
    },
};

/// One step of a posted click sequence at a single client point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PostedMouseStep {
    Move,
    Down,
    DoubleClick,
    Hold,
    Up,
    InterClickDelay,
}

/// Win32 message order for `clicks` presses at one point. The second press of a
/// pair is WM_*BUTTONDBLCLK when the window class has CS_DBLCLKS (without it the
/// system delivers a plain WM_*BUTTONDOWN); a third press starts a new pair.
pub(super) fn posted_click_steps(
    clicks: u8,
    class_accepts_double_clicks: bool,
) -> Vec<PostedMouseStep> {
    let mut steps = Vec::with_capacity(usize::from(clicks) * 4 + 1);
    steps.push(PostedMouseStep::Move);
    for click_index in 0..clicks {
        if click_index > 0 {
            steps.push(PostedMouseStep::InterClickDelay);
        }
        steps.push(if click_index % 2 == 1 && class_accepts_double_clicks {
            PostedMouseStep::DoubleClick
        } else {
            PostedMouseStep::Down
        });
        steps.push(PostedMouseStep::Hold);
        steps.push(PostedMouseStep::Up);
    }
    steps
}

/// Live identity of a PostMessage child target under its claimed root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ChildHwndIdentity {
    pub(super) hwnd: i64,
    pub(super) root_hwnd: i64,
    pub(super) descends_from_root: bool,
    pub(super) pid: u32,
    pub(super) generation: u64,
    pub(super) root_generation: u64,
}

/// Pure comparison of the identity captured at resolution against a fresh
/// readback (`None` = the child window no longer exists).
pub(super) fn revalidate_child_identity(
    expected: &ChildHwndIdentity,
    observed: Option<&ChildHwndIdentity>,
) -> Result<(), ActionError> {
    let stale = |reason: String| ActionError::TargetInvalid {
        detail: format!(
            "act_click PostMessage child HWND 0x{:x} under claimed root 0x{:x} is stale: {reason}; no further mouse messages were posted",
            expected.hwnd, expected.root_hwnd
        ),
    };
    let Some(observed) = observed else {
        return Err(stale("the window no longer exists".to_owned()));
    };
    if observed.hwnd != expected.hwnd
        || observed.root_hwnd != expected.root_hwnd
        || !observed.descends_from_root
    {
        return Err(stale(
            "it is no longer a descendant of the claimed root".to_owned(),
        ));
    }
    if observed.pid != expected.pid {
        return Err(stale(format!(
            "owner pid changed from {} to {}",
            expected.pid, observed.pid
        )));
    }
    if observed.generation != expected.generation
        || observed.root_generation != expected.root_generation
    {
        return Err(stale(format!(
            "HWND lifetime generation changed (child {}->{}, root {}->{}); the handle was destroyed and recycled",
            expected.generation,
            observed.generation,
            expected.root_generation,
            observed.root_generation
        )));
    }
    Ok(())
}

/// Captures the identity at resolution time; the child must be a live
/// descendant of `root_hwnd` (or the root itself).
#[cfg(windows)]
pub(super) fn snapshot_child_identity(
    root_hwnd: i64,
    hwnd: i64,
) -> Result<ChildHwndIdentity, ActionError> {
    match observe_child_identity(root_hwnd, hwnd) {
        Some(identity) if identity.descends_from_root => Ok(identity),
        _ => Err(ActionError::TargetInvalid {
            detail: format!(
                "act_click PostMessage child HWND 0x{hwnd:x} is not a live descendant of claimed root 0x{root_hwnd:x}; no mouse messages were posted"
            ),
        }),
    }
}

/// Re-reads the live identity of `hwnd` relative to `root_hwnd`.
#[cfg(windows)]
pub(super) fn observe_child_identity(root_hwnd: i64, hwnd: i64) -> Option<ChildHwndIdentity> {
    let child = raw_hwnd(hwnd);
    // SAFETY: HWND is a plain handle value that is never dereferenced; IsWindow only validates
    // it and returns FALSE for a stale or invalid handle.
    if child.0.is_null() || !unsafe { IsWindow(Some(child)) }.as_bool() {
        return None;
    }
    // SAFETY: IsChild only compares handle values in the window manager; stale or invalid
    // handles make it return FALSE.
    let descends_from_root =
        hwnd == root_hwnd || unsafe { IsChild(raw_hwnd(root_hwnd), child) }.as_bool();
    let mut pid = 0_u32;
    // SAFETY: the out-pointer refers to the live local `pid`; a stale HWND returns 0 and leaves
    // `pid` at 0, which then mismatches the captured pid.
    unsafe { GetWindowThreadProcessId(child, Some(&raw mut pid)) };
    Some(ChildHwndIdentity {
        hwnd,
        root_hwnd,
        descends_from_root,
        pid,
        generation: synapse_capture::window_generation(hwnd),
        root_generation: synapse_capture::window_generation(root_hwnd),
    })
}

/// Whether the child's window class receives WM_*BUTTONDBLCLK (CS_DBLCLKS).
#[cfg(windows)]
pub(super) fn class_accepts_double_clicks(hwnd: i64) -> bool {
    // SAFETY: GetClassLongW only reads class data for the handle; an invalid handle returns 0.
    let style = unsafe { GetClassLongW(raw_hwnd(hwnd), GCL_STYLE) };
    style & CS_DBLCLKS.0 != 0
}

/// Maps a WM_*BUTTONDOWN message to its WM_*BUTTONDBLCLK counterpart.
#[cfg(windows)]
pub(super) fn double_click_message(down_message: u32) -> Result<u32, ActionError> {
    match down_message {
        WM_LBUTTONDOWN => Ok(WM_LBUTTONDBLCLK),
        WM_RBUTTONDOWN => Ok(WM_RBUTTONDBLCLK),
        WM_MBUTTONDOWN => Ok(WM_MBUTTONDBLCLK),
        other => Err(ActionError::BackendUnavailable {
            detail: format!(
                "act_click PostMessage has no double-click message for button-down 0x{other:x}"
            ),
        }),
    }
}

#[cfg(windows)]
fn raw_hwnd(hwnd: i64) -> HWND {
    HWND(hwnd as isize as *mut c_void)
}
