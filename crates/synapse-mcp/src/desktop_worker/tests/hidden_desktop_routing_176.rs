use super::ensure_hwnd_on_desktop;
use crate::m1::mcp_error;
use synapse_core::error_codes;

fn code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn must_pass_window_listed_on_the_desktop_is_routed() {
    let result = ensure_hwnd_on_desktop("hidden-a", 0x30, |desktop| {
        assert_eq!(desktop, "hidden-a");
        Ok(vec![0x10, 0x30])
    });
    assert!(result.is_ok());
}

#[test]
fn must_fail_human_window_not_listed_on_the_desktop_is_refused_as_a_target_miss() {
    let error = ensure_hwnd_on_desktop("hidden-a", 0x1111, |_| Ok(vec![0x10, 0x30]))
        .expect_err("human hwnd must not reach the PrintWindow worker");
    assert_eq!(code(&error), Some(error_codes::TARGET_WINDOW_NOT_FOUND));
    assert!(error.message.contains("0x1111"), "{}", error.message);
}

#[test]
fn must_fail_empty_desktop_refuses_every_window() {
    let error = ensure_hwnd_on_desktop("hidden-a", 0x30, |_| Ok(Vec::new()))
        .expect_err("empty desktop owns nothing");
    assert_eq!(code(&error), Some(error_codes::TARGET_WINDOW_NOT_FOUND));
}

#[test]
fn must_fail_unreadable_window_list_fails_closed_as_a_target_miss() {
    let error = ensure_hwnd_on_desktop("dead", 0x30, |_| {
        Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "desktop closed",
        ))
    })
    .expect_err("unverifiable ownership must not route");
    assert_eq!(code(&error), Some(error_codes::TARGET_WINDOW_NOT_FOUND));
    assert!(
        error.message.contains("desktop closed"),
        "{}",
        error.message
    );
}
