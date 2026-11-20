//! #299: the hwnd an agent hands to the Chrome bridge must be a real browser
//! window that the bridge can tell apart from every other browser window.
//!
//! The bridge maps an hwnd to a Chrome window by its bounds, then by its title
//! when bounds tie. Two inputs made it answer for the WRONG window:
//! - a Chromium popup that is not a browser window (the 'Restore pages?'
//!   bubble shares the `Chrome_WidgetWin_1` class): it is owned by a browser
//!   window; an unowned browser window may temporarily have no title;
//! - two browser windows with the same bounds AND the same title: no hint the
//!   bridge receives can separate them.
//!
//! Both are refused before the bridge is asked, with an error that says what
//! to pass instead.

use synapse_core::{ForegroundContext, Rect, error_codes};

use super::{ErrorData, mcp_error};

/// Largest per-edge difference (px) at which two windows count as having the
/// same bounds (DPI rounding between the OS and Chrome's own window bounds).
const SAME_BOUNDS_TOLERANCE_PX: u32 = 4;

/// What is known about the requested window and the other browser windows.
pub(crate) struct BrowserWindowFacts<'a> {
    pub(crate) requested: &'a ForegroundContext,
    /// Nearest `GW_OWNER` of the requested window, when it has one.
    pub(crate) owner_hwnd: Option<i64>,
    /// The other visible, unowned top-level windows of the same browser.
    pub(crate) other_windows: &'a [ForegroundContext],
}

impl BrowserWindowFacts<'_> {
    /// Why the bridge must not be asked about this window, if it must not.
    pub(crate) fn refusal(&self, tool: &str) -> Option<ErrorData> {
        let hwnd = self.requested.hwnd;
        if let Some(owner) = self.owner_hwnd {
            return Some(refuse(format!(
                "{tool} refused window_hwnd {hwnd:#x}: it is a popup owned by window {owner:#x} (a dialog or bubble such as 'Restore pages?', same Chrome_WidgetWin_1 class), not a browser window; pass the browser window's hwnd"
            )));
        }
        let twin = self.other_windows.iter().find(|other| {
            other.window_title == self.requested.window_title
                && same_bounds(&other.window_bounds, &self.requested.window_bounds)
        })?;
        Some(refuse(format!(
            "{tool} refused window_hwnd {hwnd:#x}: browser window {:#x} has the same bounds and the same title, so the Chrome bridge cannot tell the two apart; give one of them other bounds (act verb=set_window_bounds) or another page, then retry",
            twin.hwnd
        )))
    }
}

fn same_bounds(left: &Rect, right: &Rect) -> bool {
    left.x.abs_diff(right.x) <= SAME_BOUNDS_TOLERANCE_PX
        && left.y.abs_diff(right.y) <= SAME_BOUNDS_TOLERANCE_PX
        && left.w.abs_diff(right.w) <= SAME_BOUNDS_TOLERANCE_PX
        && left.h.abs_diff(right.h) <= SAME_BOUNDS_TOLERANCE_PX
}

fn refuse(detail: String) -> ErrorData {
    mcp_error(error_codes::ACTION_TARGET_INVALID, detail)
}

/// Reads the facts for `context` from the live desktop and refuses a window
/// the Chrome bridge could map to the wrong browser window.
#[cfg(windows)]
pub(crate) fn ensure_browser_window(
    tool: &str,
    context: &ForegroundContext,
) -> Result<(), ErrorData> {
    let owner_hwnd = synapse_a11y::window_owner_chain(context.hwnd)
        .first()
        .map(|(hwnd, _pid)| *hwnd);
    let other_windows = synapse_a11y::visible_top_level_window_contexts()
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!(
                    "{tool} could not list the top-level windows needed to check the Chrome window mapping: {error}"
                ),
            )
        })?
        .into_iter()
        .filter(|window| {
            window.hwnd != context.hwnd
                && window.process_name.eq_ignore_ascii_case(&context.process_name)
                && synapse_a11y::window_owner_chain(window.hwnd).is_empty()
        })
        .collect::<Vec<_>>();
    let facts = BrowserWindowFacts {
        requested: context,
        owner_hwnd,
        other_windows: &other_windows,
    };
    facts.refusal(tool).map_or(Ok(()), Err)
}

#[cfg(test)]
#[path = "tests/browser_window_guard_299.rs"]
mod browser_window_guard_299_tests;
