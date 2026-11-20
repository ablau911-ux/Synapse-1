//! Regression tests for #222: the background WebView2 click refusal (#79) also
//! fires for a session target that was never claimed, so its text must not
//! assert that the HWND is "claimed".

use super::super::background_router::webview2_background_click_refusal_message;

#[test]
fn must_fail_refusal_text_does_not_call_the_target_claimed() {
    let message = webview2_background_click_refusal_message(0x1a2b, None);
    assert!(!message.contains("claimed"), "message: {message}");
    assert!(message.contains("target HWND 0x1a2b"), "message: {message}");
    assert!(message.contains("before delivery"), "message: {message}");
}

#[test]
fn must_pass_refusal_text_keeps_the_diagnostic_detail_or_the_launch_hint() {
    let with_detail = webview2_background_click_refusal_message(7, Some("CDP port closed"));
    assert!(
        with_detail.ends_with("CDP port closed"),
        "message: {with_detail}"
    );
    let without = webview2_background_click_refusal_message(7, None);
    assert!(without.contains("cdp_debug=true"), "message: {without}");
}
