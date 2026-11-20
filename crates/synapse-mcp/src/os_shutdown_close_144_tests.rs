//! #144: a plain `taskkill /PID` posts WM_CLOSE to the hidden session window.
//! DefWindowProc would destroy it, silently disabling later OS-shutdown records.
use windows::Win32::UI::WindowsAndMessaging::{
    WM_CLOSE, WM_DESTROY, WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION, WM_QUIT,
};

use super::window_message_reason;

#[test]
fn must_pass_wm_close_is_a_graceful_close_request() {
    assert_eq!(window_message_reason(WM_CLOSE), Some("close_request"));
}

/// Guard (source scan, no dispatch seam yet): `window_proc` must route every
/// message through `window_message_reason` into `exit_before_deadline` BEFORE
/// the `match` whose default arm is `DefWindowProcW`. Otherwise WM_CLOSE would
/// reach DefWindowProc and destroy the hidden window while the helper test
/// above still passes.
#[test]
fn must_fail_window_proc_never_lets_wm_close_reach_def_window_proc() {
    let source = include_str!("os_shutdown.rs");
    let start = source
        .find("fn window_proc(")
        .unwrap_or_else(|| panic!("window_proc missing from os_shutdown.rs"));
    let body = &source[start..];
    let end = body
        .find("\nfn ")
        .unwrap_or_else(|| panic!("window_proc end not found"));
    let body = &body[..end];
    let code: String = body
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let reason_at = code
        .find("window_message_reason(message)")
        .unwrap_or_else(|| panic!("window_proc does not consult window_message_reason"));
    let exit_at = code
        .find("exit_before_deadline(reason)")
        .unwrap_or_else(|| panic!("window_proc does not exit on a close reason"));
    let match_at = code
        .find("match message")
        .unwrap_or_else(|| panic!("window_proc dispatch match missing"));
    let default_at = code
        .find("DefWindowProcW(")
        .unwrap_or_else(|| panic!("window_proc default arm missing"));
    assert!(reason_at < exit_at, "reason must be checked before exiting");
    assert!(
        exit_at < match_at && exit_at < default_at,
        "close-request exit must happen before the match that falls to DefWindowProcW"
    );
}

#[test]
fn must_fail_other_messages_do_not_request_exit() {
    for message in [
        WM_DESTROY,
        WM_NCDESTROY,
        WM_QUIT,
        WM_QUERYENDSESSION,
        WM_ENDSESSION, // handled separately through session_reason (needs wParam/lParam)
        0,
        u32::MAX,
    ] {
        assert_eq!(window_message_reason(message), None, "message {message:#x}");
    }
}
