//! Live root HWND and process identity for OS-input delivery.
use crate::{A11yError, A11yResult, WindowRootIdentity};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetForegroundWindow, GetWindowThreadProcessId,
    },
};

use super::valid_hwnd;

pub fn window_root_identity(hwnd: i64) -> A11yResult<WindowRootIdentity> {
    root_identity_for_native(valid_hwnd(hwnd)?, "target")
}

/// Reads the live OS foreground window as a top-level root HWND + pid.
///
/// # Errors
///
/// Returns `A11Y_NO_FOREGROUND` when Windows reports no foreground window.
pub fn foreground_root_identity() -> A11yResult<WindowRootIdentity> {
    // SAFETY: GetForegroundWindow has no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return Err(A11yError::NoForeground {
            detail: "GetForegroundWindow returned null".to_owned(),
        });
    }
    root_identity_for_native(foreground, "foreground").map_err(crate::as_foreground_error)
}

fn root_identity_for_native(hwnd: HWND, role: &'static str) -> A11yResult<WindowRootIdentity> {
    // SAFETY: GetAncestor reads USER state for a handle value.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    let mut pid = 0_u32;
    // SAFETY: `pid` is a valid out pointer for the duration of the call.
    let thread = unsafe { GetWindowThreadProcessId(root, Some(&raw mut pid)) };
    if thread == 0 || pid == 0 {
        return Err(A11yError::WindowNotFound {
            detail: format!(
                "{role} root HWND {:#x} owner process could not be read",
                root.0 as isize
            ),
        });
    }
    Ok(WindowRootIdentity {
        root_hwnd: root.0 as isize as i64,
        pid,
    })
}
