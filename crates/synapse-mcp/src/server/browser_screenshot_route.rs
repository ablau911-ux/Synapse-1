//! Which lane `browser_screenshot` uses for a session-owned Chrome tab (#123,
//! #177, upstream 847cb1bc). The decisions are pure and table-tested in
//! `browser_screenshot_route_table_123.rs`; the OS reads (foreground window,
//! Chrome command line) and the bridge call stay in `m1_tools.rs`.

use serde_json::{Value, json};

/// Bridge tier the worker reports for a `chrome.debugger`
/// `Page.captureScreenshot` capture.
pub(super) const BACKGROUND_CAPTURE_TIER: &str = "chrome_debugger_background_capture";

/// The lane one capture takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BrowserScreenshotRoute {
    /// Chrome is the OS foreground: the foreground-verified capture. With
    /// `debugger_allowed` the worker may render an inactive or hidden tab
    /// through `chrome.debugger` instead of refusing.
    Foreground { debugger_allowed: bool },
    /// Chrome is not the OS foreground and its debugger is banner-free:
    /// capture through `chrome.debugger`, never touch focus.
    Background,
    /// Refused before any bridge call.
    Refuse,
}

/// Chooses the lane from the OS foreground window, the Chrome window that
/// hosts the tab, and whether a debugger attach there is banner-free.
pub(super) fn browser_screenshot_route(
    foreground_hwnd: Option<i64>,
    chrome_hwnd: i64,
    debugger_banner_free: bool,
) -> BrowserScreenshotRoute {
    if chrome_hwnd == 0 {
        BrowserScreenshotRoute::Refuse
    } else if foreground_hwnd == Some(chrome_hwnd) {
        BrowserScreenshotRoute::Foreground {
            debugger_allowed: debugger_banner_free,
        }
    } else if debugger_banner_free {
        BrowserScreenshotRoute::Background
    } else {
        BrowserScreenshotRoute::Refuse
    }
}

/// The worker's `background` field for a route: `true` = debugger capture,
/// `"auto"` = visible tab when it is active in the focused window, else
/// debugger, `false` = visible tab only.
pub(super) fn browser_screenshot_worker_background(route: BrowserScreenshotRoute) -> Value {
    match route {
        BrowserScreenshotRoute::Foreground {
            debugger_allowed: true,
        } => json!("auto"),
        BrowserScreenshotRoute::Background => json!(true),
        BrowserScreenshotRoute::Foreground {
            debugger_allowed: false,
        }
        | BrowserScreenshotRoute::Refuse => json!(false),
    }
}

/// Whether a bridge disconnect may fall back to a whole-window WGC image.
/// That image shows Chrome's active tab, so it is never used when the
/// capture did not require the target to be the visible tab.
pub(super) const fn browser_screenshot_allows_window_fallback(
    route: BrowserScreenshotRoute,
) -> bool {
    matches!(route, BrowserScreenshotRoute::Foreground { .. })
}

/// `(capture_backend, source_of_truth)` for the tier the worker reported.
pub(super) fn browser_screenshot_labels(backend_tier_used: &str) -> (&'static str, &'static str) {
    if backend_tier_used == BACKGROUND_CAPTURE_TIER {
        (
            "chrome_debugger_capture_screenshot_stitched_bgra",
            "human OS foreground readback (no focus call) plus normal Chrome bridge chrome.scripting page metrics/masks and chrome.debugger Page.captureScreenshot chunks stitched by synapse-mcp",
        )
    } else {
        (
            "chrome_tabs_capture_visible_tab_stitched_bgra",
            "human OS foreground readback plus normal Chrome bridge chrome.scripting page metrics/masks/scroll and chrome.tabs.captureVisibleTab tiles stitched by synapse-mcp",
        )
    }
}

#[cfg(test)]
#[path = "browser_screenshot_route_table_123.rs"]
mod table_123;
