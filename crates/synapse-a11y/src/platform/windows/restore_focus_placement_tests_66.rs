//! #66: focus recovery must restore ONLY a minimized window. `SW_RESTORE` on a
//! maximized (or normal) window rewrites the user's placement, so
//! `restore_window_for_focus` must leave non-iconic windows untouched and must
//! be a no-op on null / destroyed handles.
//!
//! Contract: issue #66 "Acceptance" (exact HWND, IsZoomed, IsIconic, show state
//! and outer bounds unchanged for maximized and normal windows; invalid/closed
//! HWND cannot resize unrelated windows); window.rs `restore_window_for_focus`
//! comment ("Only minimized windows need restoration").
//!
//! Fixture: hidden (never shown) test-owned STATIC windows whose placement is
//! set by creation style, so the guarded path never shows or activates
//! anything; visibility is part of the readback (an unguarded `SW_RESTORE`
//! would show the window). The minimized case is restored by design.

use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetWindowRect, IsIconic, IsWindowVisible, IsZoomed,
            WINDOW_EX_STYLE, WINDOW_STYLE, WS_MAXIMIZE, WS_MINIMIZE, WS_OVERLAPPEDWINDOW,
        },
    },
    core::w,
};

use super::restore_window_for_focus;

struct Placed {
    hwnd: HWND,
}

impl Placed {
    fn hidden(extra: WINDOW_STYLE) -> Self {
        // SAFETY: built-in class, no create params; destroyed on drop on this thread.
        let created = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("synapse-a11y-66-placement"),
                WS_OVERLAPPEDWINDOW | extra,
                40,
                40,
                320,
                200,
                None,
                None,
                None,
                None,
            )
        };
        match created {
            Ok(hwnd) => Self { hwnd },
            Err(error) => panic!("test window must be created: {error}"),
        }
    }

    fn readback(&self) -> (bool, bool, bool, (i32, i32, i32, i32)) {
        let mut rect = RECT::default();
        // SAFETY: valid HWND owned by this guard; out-pointer to a local RECT.
        unsafe { GetWindowRect(self.hwnd, &raw mut rect) }.expect("GetWindowRect");
        // SAFETY: read-only queries on a valid HWND.
        let (zoomed, iconic, visible) = unsafe {
            (
                IsZoomed(self.hwnd).as_bool(),
                IsIconic(self.hwnd).as_bool(),
                IsWindowVisible(self.hwnd).as_bool(),
            )
        };
        (
            zoomed,
            iconic,
            visible,
            (rect.left, rect.top, rect.right, rect.bottom),
        )
    }
}

impl Drop for Placed {
    fn drop(&mut self) {
        // SAFETY: destroys the window this guard created, on the same thread.
        let _destroyed = unsafe { DestroyWindow(self.hwnd) };
    }
}

#[test]
fn must_pass_maximized_window_keeps_its_exact_placement() {
    let window = Placed::hidden(WS_MAXIMIZE);
    let before = window.readback();
    assert!(
        before.0 && !before.1 && !before.2,
        "precondition: hidden maximized {before:?}"
    );

    restore_window_for_focus(window.hwnd);

    assert_eq!(
        window.readback(),
        before,
        "maximized placement must not change"
    );
}

#[test]
fn must_pass_normal_window_keeps_its_exact_placement() {
    let window = Placed::hidden(WINDOW_STYLE(0));
    let before = window.readback();
    assert!(
        !before.0 && !before.1 && !before.2,
        "precondition: hidden normal {before:?}"
    );

    restore_window_for_focus(window.hwnd);

    assert_eq!(
        window.readback(),
        before,
        "normal placement must not change"
    );
}

/// `SW_RESTORE` shows AND may activate the window; a test process started from
/// the owner's terminal can be allowed to take the foreground, so this runs
/// only on demand (`--ignored`) during the live VBV session.
#[test]
#[ignore = "SW_RESTORE may activate the test window and take the owner's foreground"]
fn must_pass_minimized_window_is_restored() {
    let window = Placed::hidden(WS_MINIMIZE);
    assert!(window.readback().1, "precondition: minimized");

    restore_window_for_focus(window.hwnd);

    assert!(
        !window.readback().1,
        "a minimized window is restored for focus"
    );
}

#[test]
fn must_fail_null_or_destroyed_hwnd_changes_nothing() {
    let sentinel = Placed::hidden(WS_MAXIMIZE);
    let before = sentinel.readback();
    let destroyed = Placed::hidden(WINDOW_STYLE(0));
    let stale = destroyed.hwnd;
    drop(destroyed);

    restore_window_for_focus(HWND(std::ptr::null_mut()));
    restore_window_for_focus(stale);

    assert_eq!(sentinel.readback(), before, "unrelated window untouched");
}
