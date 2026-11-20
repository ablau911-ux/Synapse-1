//! #217: `window_owner_chain` feeds the foreground-restore own-focus decision.
//! A null or invalid HWND must yield an empty chain and never panic; a real
//! owned window must list its owner.

use windows::{
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_POPUP,
        },
    },
    core::w,
};

use super::window_owner_chain;

/// A hidden (never shown) top-level `STATIC` window, destroyed on drop. Shared
/// with the #223 DPI tests.
pub(super) struct TestWindow {
    pub(super) hwnd: HWND,
}

impl TestWindow {
    /// Creates a hidden window, optionally owned by `owner`.
    pub(super) fn hidden(owner: Option<HWND>) -> Self {
        // SAFETY: a built-in class, no create params; the handle is owned by
        // this guard and destroyed on drop on the creating thread.
        let created = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("synapse-a11y-test-window"),
                if owner.is_some() {
                    WS_POPUP
                } else {
                    WINDOW_STYLE(0)
                },
                0,
                0,
                64,
                64,
                owner,
                None,
                None,
                None,
            )
        };
        match created {
            Ok(hwnd) => Self { hwnd },
            Err(error) => panic!("hidden test window must be created: {error}"),
        }
    }

    pub(super) fn raw(&self) -> i64 {
        self.hwnd.0 as isize as i64
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: destroys the window this guard created, on the same thread.
        let _destroyed = unsafe { DestroyWindow(self.hwnd) };
    }
}

#[test]
fn must_pass_a_null_hwnd_has_an_empty_owner_chain() {
    assert!(window_owner_chain(0).is_empty());
}

#[test]
fn must_fail_an_invalid_hwnd_yields_an_empty_chain_without_panicking() {
    // Handles that name no live window: GetWindow fails, the walk stops.
    for hwnd in [0x7FFF_FFF0_i64, 0x0FFF_FFFE] {
        assert!(
            window_owner_chain(hwnd).is_empty(),
            "invalid hwnd 0x{hwnd:x} must have no owner chain"
        );
    }
}

#[test]
fn must_pass_owned_window_chain_lists_its_owner() {
    let owner = TestWindow::hidden(None);
    let owned = TestWindow::hidden(Some(owner.hwnd));

    assert_eq!(
        window_owner_chain(owned.raw()),
        vec![(owner.raw(), std::process::id())],
        "the owned window's chain is exactly [its owner, in this process]"
    );
    // The owner itself is unowned: a live window with no owner has no chain.
    assert!(window_owner_chain(owner.raw()).is_empty());

    drop(owned);
    drop(owner);
}

#[test]
fn must_pass_two_level_owner_chain_is_nearest_owner_first() {
    let root = TestWindow::hidden(None);
    let middle = TestWindow::hidden(Some(root.hwnd));
    let leaf = TestWindow::hidden(Some(middle.hwnd));
    let pid = std::process::id();

    assert_eq!(
        window_owner_chain(leaf.raw()),
        vec![(middle.raw(), pid), (root.raw(), pid)]
    );

    drop(leaf);
    drop(middle);
    drop(root);
}
