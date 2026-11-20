//! Hidden-desktop routing decision (#176).
//!
//! HWNDs are valid session-wide, so a desktop worker's `Context` probe succeeds for any live
//! window, including the human's. A target may only be sent to a hidden-desktop worker
//! (PrintWindow path) when it is actually a window of that hidden desktop.
//!
//! `GetThreadDesktop` cannot answer this from the daemon: it only works for threads of the
//! calling process. The cross-process source of truth is the desktop's own window list
//! (`EnumDesktopWindows`, exposed as `hidden_desktop_window_hwnds`), so the decision is
//! membership of the HWND in that list.

use rmcp::ErrorData;
use synapse_core::error_codes;

/// Fails with `TARGET_WINDOW_NOT_FOUND` unless `hwnd` is a window of `desktop_name`.
///
/// `TARGET_WINDOW_NOT_FOUND` is the miss signal callers already treat as "not on this desktop,
/// try the next one / fall back to the normal path". A window list that cannot be read (for
/// example the desktop was already closed) fails closed the same way: the window is not provably
/// on that desktop, so no worker may touch it.
pub(super) fn ensure_hwnd_on_desktop(
    desktop_name: &str,
    hwnd: i64,
    list_desktop_hwnds: impl FnOnce(&str) -> Result<Vec<i64>, ErrorData>,
) -> Result<(), ErrorData> {
    let reason = match list_desktop_hwnds(desktop_name) {
        Ok(hwnds) if hwnds.contains(&hwnd) => return Ok(()),
        Ok(_) => "is not a window of that desktop".to_owned(),
        Err(error) => format!(
            "could not be verified (window list unreadable: {})",
            error.message
        ),
    };
    Err(crate::m1::mcp_error(
        error_codes::TARGET_WINDOW_NOT_FOUND,
        format!(
            "hwnd {hwnd:#x} {reason}; hidden desktop {desktop_name:?} refuses to route it to a worker"
        ),
    ))
}

#[cfg(test)]
#[path = "tests/hidden_desktop_routing_176.rs"]
mod tests;
