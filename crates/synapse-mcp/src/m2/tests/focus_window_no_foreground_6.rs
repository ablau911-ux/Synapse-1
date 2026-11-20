//! #6 regression: authorized focus recovery must work when the OS has no
//! foreground window. The missing prior foreground is tolerated only when the
//! caller proved the exact session claim; every other read failure still fails.

use serde_json::Value;
use synapse_core::{ForegroundContext, Rect, error_codes};

use super::{before_foreground_for_focus, focus_activation_plan};

fn context(hwnd: i64) -> ForegroundContext {
    ForegroundContext {
        hwnd,
        pid: 42,
        process_name: "notepad.exe".to_owned(),
        process_path: r"C:\Windows\notepad.exe".to_owned(),
        window_title: "Notes".to_owned(),
        window_bounds: Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        },
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: None,
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    }
}

fn no_foreground() -> synapse_a11y::A11yError {
    synapse_a11y::A11yError::NoForeground {
        detail: "GetForegroundWindow returned null".to_owned(),
    }
}

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

#[test]
fn must_pass_proven_recovery_tolerates_missing_foreground() {
    let before = before_foreground_for_focus(Err(no_foreground()), true)
        .expect("proven recovery must accept a missing foreground");
    assert_eq!(before.hwnd, 0, "missing foreground reads back as hwnd 0");
    assert!(before.window_title.is_empty());
    assert_eq!(focus_activation_plan(before.hwnd, 0x2002).attempts, 1);
}

#[test]
fn must_fail_unproven_recovery_still_fails_on_missing_foreground() {
    let error = before_foreground_for_focus(Err(no_foreground()), false)
        .expect_err("unproven focus must fail closed without a foreground");
    assert_eq!(error_code(&error), Some(error_codes::A11Y_NO_FOREGROUND));
}

#[test]
fn must_fail_proven_recovery_still_fails_on_other_read_errors() {
    let error = before_foreground_for_focus(Err(synapse_a11y::A11yError::internal("boom")), true)
        .expect_err("only A11Y_NO_FOREGROUND is tolerated");
    assert_eq!(error_code(&error), Some(error_codes::OBSERVE_INTERNAL));
}

#[test]
fn must_pass_present_foreground_is_returned_unchanged() {
    let before = before_foreground_for_focus(Ok(context(0x1001)), true).expect("ok passes");
    assert_eq!(before.hwnd, 0x1001);
    let before = before_foreground_for_focus(Ok(context(0x1001)), false).expect("ok passes");
    assert_eq!(before.window_title, "Notes");
}
