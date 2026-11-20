//! #299 MY TDD contract: the Chrome bridge is never asked about a window it
//! could map to the wrong browser window: an owned popup or bubble, or a window
//! whose bounds and title another browser window shares. An unowned window may
//! have no title while loading. Browser windows with the same bounds but a
//! different title (the bridge's title tie-break), pass.

use serde_json::Value;
use synapse_core::{ForegroundContext, Rect, error_codes};

use super::BrowserWindowFacts;

fn window(hwnd: i64, title: &str, bounds: (i32, i32, i32, i32)) -> ForegroundContext {
    ForegroundContext {
        hwnd,
        pid: 4242,
        process_name: "chrome.exe".to_owned(),
        process_path: r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_owned(),
        window_title: title.to_owned(),
        window_bounds: Rect {
            x: bounds.0,
            y: bounds.1,
            w: bounds.2,
            h: bounds.3,
        },
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: None,
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    }
}

fn refusal_code(facts: &BrowserWindowFacts<'_>) -> Option<(String, String)> {
    facts.refusal("browser_tabs").map(|error| {
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        (code, error.message.to_string())
    })
}

const MAXIMIZED: (i32, i32, i32, i32) = (-8, -8, 1936, 1056);

#[test]
fn must_fail_an_owned_popup_such_as_the_restore_pages_bubble_is_refused() {
    let bubble = window(0x5a01, "", (1500, 60, 380, 160));
    let facts = BrowserWindowFacts {
        requested: &bubble,
        owner_hwnd: Some(0x5a00),
        other_windows: &[],
    };
    let (code, message) = refusal_code(&facts).expect("an owned popup must be refused");
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID);
    assert!(
        message.contains("0x5a00") && message.contains("0x5a01"),
        "the refusal names the popup and its owner browser window: {message}"
    );
}

#[test]
fn must_pass_a_single_unowned_window_without_a_title() {
    let untitled = window(0x5a02, "", (0, 0, 1200, 900));
    let facts = BrowserWindowFacts {
        requested: &untitled,
        owner_hwnd: None,
        other_windows: &[],
    };
    assert!(
        refusal_code(&facts).is_none(),
        "a single unowned browser window may have no title while loading"
    );
}

#[test]
fn must_fail_two_windows_with_the_same_bounds_and_title_are_refused() {
    let mine = window(0x5a03, "Sign In - GMA16 - Google Chrome", MAXIMIZED);
    let twin = window(
        0x5a04,
        "Sign In - GMA16 - Google Chrome",
        (-6, -8, 1936, 1057),
    );
    let facts = BrowserWindowFacts {
        requested: &mine,
        owner_hwnd: None,
        other_windows: std::slice::from_ref(&twin),
    };
    let (code, message) = refusal_code(&facts).expect("indistinguishable windows are refused");
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID);
    assert!(message.contains("0x5a04"), "{message}");
}

#[test]
fn must_pass_real_browser_windows_the_bridge_can_tell_apart() {
    let mine = window(0x5a05, "Sign In - GMA16 - Google Chrome", MAXIMIZED);
    let other_title = window(0x5a06, "Cards - GMA16 - Google Chrome", MAXIMIZED);
    let other_bounds = window(
        0x5a07,
        "Sign In - GMA16 - Google Chrome",
        (-8, -8, 1931, 1056),
    );
    let cascaded = window(
        0x5a08,
        "Sign In - GMA16 - Google Chrome",
        (22, 22, 1200, 900),
    );
    for others in [
        vec![],
        vec![other_title],
        vec![other_bounds],
        vec![cascaded],
    ] {
        let facts = BrowserWindowFacts {
            requested: &mine,
            owner_hwnd: None,
            other_windows: &others,
        };
        assert!(
            facts.refusal("browser_tabs").is_none(),
            "a real, distinguishable browser window must pass: others={others:?}"
        );
    }
}
