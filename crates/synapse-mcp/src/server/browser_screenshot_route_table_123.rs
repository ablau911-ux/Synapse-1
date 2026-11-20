//! #123/#177 frozen behaviour table (manager-owned, work method v3; upstream
//! 847cb1bc). Codex and Opus never edit this file. One case per L row of the
//! item 1 table in the chrome-capture lane report.

use super::{
    BACKGROUND_CAPTURE_TIER, BrowserScreenshotRoute, browser_screenshot_allows_window_fallback,
    browser_screenshot_labels, browser_screenshot_route, browser_screenshot_worker_background,
};
use serde_json::{Value, json};

const CHROME: i64 = 0x00C0_FFEE;
const OTHER: i64 = 0x0D_0D0D;

const VISIBLE_TAB_BACKEND: &str = "chrome_tabs_capture_visible_tab_stitched_bgra";
const DEBUGGER_BACKEND: &str = "chrome_debugger_capture_screenshot_stitched_bgra";

#[test]
fn browser_screenshot_route_table_123() {
    use BrowserScreenshotRoute::{Background, Foreground, Refuse};
    let mut failures = Vec::new();

    // 1b: (row, OS foreground hwnd, Chrome hwnd, debugger banner-free, expected route)
    let routes: [(&str, Option<i64>, i64, bool, BrowserScreenshotRoute); 7] = [
        (
            "1b_chrome_in_front_banner_free_may_use_debugger",
            Some(CHROME),
            CHROME,
            true,
            Foreground {
                debugger_allowed: true,
            },
        ),
        (
            "1b_neighbour_chrome_in_front_owner_chrome_visible_tab_only",
            Some(CHROME),
            CHROME,
            false,
            Foreground {
                debugger_allowed: false,
            },
        ),
        (
            "1b_other_window_in_front_banner_free_is_background",
            Some(OTHER),
            CHROME,
            true,
            Background,
        ),
        (
            "1b_no_foreground_locked_screen_banner_free_is_background",
            None,
            CHROME,
            true,
            Background,
        ),
        (
            "1b_someone_else_owner_chrome_in_background_is_refused_never_bannered",
            Some(OTHER),
            CHROME,
            false,
            Refuse,
        ),
        (
            "1b_hostile_zero_chrome_hwnd_is_refused",
            Some(0),
            0,
            true,
            Refuse,
        ),
        (
            "1b_hostile_zero_chrome_hwnd_no_foreground_is_refused",
            None,
            0,
            true,
            Refuse,
        ),
    ];
    for (row, foreground, chrome, banner_free, expected) in routes {
        let got = browser_screenshot_route(foreground, chrome, banner_free);
        if got != expected {
            failures.push(format!("row {row}: route {got:?}, expected {expected:?}"));
        }
    }

    // worker flag: (row, route, expected `background` payload value)
    let flags: [(&str, BrowserScreenshotRoute, Value); 4] = [
        (
            "1b_flag_foreground_banner_free_is_auto",
            Foreground {
                debugger_allowed: true,
            },
            json!("auto"),
        ),
        (
            "1b_flag_neighbour_owner_chrome_is_false",
            Foreground {
                debugger_allowed: false,
            },
            json!(false),
        ),
        ("1b_flag_background_is_true", Background, json!(true)),
        ("1b_flag_refuse_is_false", Refuse, json!(false)),
    ];
    for (row, route, expected) in flags {
        let got = browser_screenshot_worker_background(route);
        if got != expected {
            failures.push(format!("row {row}: background {got}, expected {expected}"));
        }
    }

    // 1d: (row, backend tier, expected capture_backend, source_of_truth must mention)
    let labels: [(&str, &str, &str, &str); 4] = [
        (
            "1d_debugger_tier_is_labelled_debugger",
            BACKGROUND_CAPTURE_TIER,
            DEBUGGER_BACKEND,
            "chrome.debugger Page.captureScreenshot",
        ),
        (
            "1d_neighbour_visible_tab_tier_unchanged",
            "chrome_tabs_extension",
            VISIBLE_TAB_BACKEND,
            "chrome.tabs.captureVisibleTab",
        ),
        (
            "1d_empty_tier_is_visible_tab",
            "",
            VISIBLE_TAB_BACKEND,
            "chrome.tabs.captureVisibleTab",
        ),
        (
            "1d_hostile_near_miss_tier_is_visible_tab",
            "CHROME_DEBUGGER_BACKGROUND_CAPTURE ",
            VISIBLE_TAB_BACKEND,
            "chrome.tabs.captureVisibleTab",
        ),
    ];
    for (row, tier, backend, source_mentions) in labels {
        let (got_backend, got_source) = browser_screenshot_labels(tier);
        if got_backend != backend || !got_source.contains(source_mentions) {
            failures.push(format!(
                "row {row}: labels ({got_backend:?}, {got_source:?}), expected backend {backend:?} with source mentioning {source_mentions:?}"
            ));
        }
    }

    // 1i: (row, route, whole-window WGC fallback allowed on bridge disconnect)
    let fallbacks: [(&str, BrowserScreenshotRoute, bool); 3] = [
        (
            "1i_background_disconnect_never_falls_back_to_window_image",
            Background,
            false,
        ),
        (
            "1i_neighbour_foreground_disconnect_keeps_window_fallback",
            Foreground {
                debugger_allowed: false,
            },
            true,
        ),
        ("1i_refuse_never_falls_back", Refuse, false),
    ];
    for (row, route, expected) in fallbacks {
        let got = browser_screenshot_allows_window_fallback(route);
        if got != expected {
            failures.push(format!("row {row}: fallback {got}, expected {expected}"));
        }
    }

    assert!(
        failures.is_empty(),
        "#123 browser_screenshot route table rows failed:\n{}",
        failures.join("\n")
    );
}
