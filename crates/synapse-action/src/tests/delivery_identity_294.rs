//! #294: delivery identity is root HWND + pid. A closed window or a root HWND
//! reused by another process is refused, never silently retargeted.
use crate::{ActionError, InputDeliveryTarget};

const ROOT: i64 = 0x11;

fn identity(root_hwnd: i64, pid: u32, source: &'static str) -> InputDeliveryTarget {
    InputDeliveryTarget {
        root_hwnd,
        pid,
        source,
    }
}

#[test]
fn must_pass_matching_bound_target_keeps_its_proven_identity() -> Result<(), ActionError> {
    let bound = identity(ROOT, 101, "act_focus_window");
    let kept =
        identity(ROOT, 101, "explicit_window_hwnd").keep_bound_identity(Some(bound.clone()))?;
    assert_eq!(kept, bound);
    Ok(())
}

#[test]
fn must_pass_live_identity_used_when_bound_target_is_another_window() -> Result<(), ActionError> {
    let live = identity(ROOT, 101, "explicit_window_hwnd");
    for bound in [None, Some(identity(0x22, 202, "session_target"))] {
        assert_eq!(live.clone().keep_bound_identity(bound)?, live);
    }
    Ok(())
}

#[test]
fn must_fail_root_hwnd_reused_by_another_pid() {
    let bound = identity(ROOT, 101, "act_focus_window");
    let result = identity(ROOT, 202, "foreground_type").keep_bound_identity(Some(bound));
    assert!(
        matches!(result, Err(ActionError::TargetInvalid { .. })),
        "a reused HWND must be refused as an invalid target: {result:?}"
    );
}

#[cfg(windows)]
#[test]
fn must_fail_dead_or_null_hwnd_is_target_invalid() {
    // #315: a null HWND stays ACTION_TARGET_INVALID; a closed (dead) window
    // is TARGET_WINDOW_NOT_FOUND. Both are refused before delivery.
    let null = InputDeliveryTarget::resolve_window(0, "test_294");
    assert!(
        matches!(null, Err(ActionError::TargetInvalid { .. })),
        "HWND 0x0 must be refused before delivery: {null:?}"
    );
    let dead = InputDeliveryTarget::resolve_window(0x7FFF_FFF0, "test_294");
    assert!(
        matches!(dead, Err(ActionError::WindowNotFound { .. })),
        "HWND 0x7ffffff0 must be refused before delivery as a closed window: {dead:?}"
    );
}
