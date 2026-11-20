//! #197 regression: PostMessage child hit-test must follow z-order and skip
//! invisible, disabled and `WS_EX_TRANSPARENT` windows. The old resolution
//! picked the smallest-area visible descendant containing the point, so a small
//! window buried under a docked panel received clicks and wheel events meant for
//! the panel the user actually sees.

use crate::m2::click::hit_test::{HitCandidate, topmost_hit_index};

fn hit() -> HitCandidate {
    HitCandidate {
        contains_point: true,
        visible: true,
        enabled: true,
        transparent: false,
    }
}

fn miss() -> HitCandidate {
    HitCandidate {
        contains_point: false,
        ..hit()
    }
}

#[test]
fn must_pass_topmost_overlapping_sibling_wins_over_a_smaller_one_beneath() {
    // Index 0 is the topmost sibling (a large docked panel); index 1 is a small
    // canvas child it covers. Both contain the point; the visible one is index 0.
    assert_eq!(topmost_hit_index(&[hit(), hit()]), Some(0));
}

#[test]
fn must_fail_disabled_top_sibling_is_skipped() {
    let disabled = HitCandidate {
        enabled: false,
        ..hit()
    };
    assert_eq!(topmost_hit_index(&[disabled, hit()]), Some(1));
}

#[test]
fn must_fail_invisible_top_sibling_is_skipped() {
    let hidden = HitCandidate {
        visible: false,
        ..hit()
    };
    assert_eq!(topmost_hit_index(&[hidden, hit()]), Some(1));
}

#[test]
fn must_fail_transparent_top_sibling_is_skipped() {
    let transparent = HitCandidate {
        transparent: true,
        ..hit()
    };
    assert_eq!(topmost_hit_index(&[transparent, hit()]), Some(1));
}

#[test]
fn must_fail_sibling_not_containing_the_point_is_skipped() {
    assert_eq!(topmost_hit_index(&[miss(), miss(), hit()]), Some(2));
}

#[test]
fn must_fail_no_hittable_child_returns_none_so_the_parent_is_the_target() {
    let disabled = HitCandidate {
        enabled: false,
        ..hit()
    };
    assert_eq!(topmost_hit_index(&[miss(), disabled]), None);
    assert_eq!(topmost_hit_index(&[]), None);
}

/// Real Win32 windows for the z-order walk in `deepest_hit_hwnd`
/// (`GW_CHILD` / `GW_HWNDNEXT` plus the style-bit reads), which the pure
/// `topmost_hit_index` tests above never reach.
#[cfg(windows)]
mod real_z_order {
    use synapse_core::Point;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, GetWindowRect, HWND_BOTTOM, HWND_TOP, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WS_CHILD,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };
    use windows::core::w;

    use crate::m2::click::hit_test::deepest_hit_hwnd;
    use crate::test_support::{PumpedWindows, from_hwnd, to_hwnd};

    /// Top-level parent on a message-pumping UI thread, so a concurrent UIA
    /// query that reaches it is answered. Children are created and re-ordered
    /// on that thread; dropping the parent destroys it and its children there.
    struct Parent {
        ui: PumpedWindows,
    }

    impl Parent {
        fn hwnd(&self) -> HWND {
            to_hwnd(self.ui.hwnd(0))
        }

        fn child(&self, x: i32, y: i32, size: i32) -> HWND {
            let parent = self.ui.hwnd(0);
            let created = self.ui.run(move || create_child(parent, x, y, size));
            match created {
                Ok(Ok(hwnd)) => to_hwnd(hwnd),
                Ok(Err(error)) => panic!("CreateWindowExW(child) failed: {error}"),
                Err(error) => panic!("UI thread job failed: {error}"),
            }
        }

        fn place(&self, hwnd: HWND, insert_after: HWND) {
            let (hwnd, insert_after) = (from_hwnd(hwnd), from_hwnd(insert_after));
            let placed = self.ui.run(move || {
                // SAFETY: live handle owned by this UI thread; only the z-order changes.
                unsafe {
                    SetWindowPos(
                        to_hwnd(hwnd),
                        Some(to_hwnd(insert_after)),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    )
                }
                .map_err(|error| error.to_string())
            });
            match placed {
                Ok(Ok(())) => {}
                Ok(Err(error)) => panic!("SetWindowPos failed: {error}"),
                Err(error) => panic!("UI thread job failed: {error}"),
            }
        }
    }

    fn parent() -> Parent {
        let ui = PumpedWindows::spawn(|| {
            // SAFETY: STATIC is a system class; no Rust pointer is handed to Windows.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("synapse hit test 197 parent"),
                    WS_POPUP,
                    -16_000,
                    -16_000,
                    300,
                    300,
                    None,
                    None,
                    None,
                    None,
                )
            }?;
            // SAFETY: live handle owned by this thread. A child is only visible
            // (IsWindowVisible) when its parent is, so the parent must be shown.
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            Ok(vec![from_hwnd(hwnd)])
        });
        match ui {
            Ok(ui) => Parent { ui },
            Err(error) => panic!("CreateWindowExW(parent) failed: {error:#}"),
        }
    }

    fn create_child(parent: i64, x: i32, y: i32, size: i32) -> Result<i64, String> {
        // SAFETY: STATIC is a system class; `parent` is a live window on this thread.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_VISIBLE,
                x,
                y,
                size,
                size,
                Some(to_hwnd(parent)),
                None,
                None,
                None,
            )
        }
        .map(from_hwnd)
        .map_err(|error| error.to_string())
    }

    fn inside(hwnd: HWND) -> Point {
        let mut rect = RECT::default();
        // SAFETY: the out-pointer refers to the live local `rect`.
        if let Err(error) = unsafe { GetWindowRect(hwnd, &raw mut rect) } {
            panic!("GetWindowRect failed: {error}");
        }
        Point {
            x: rect.left + 5,
            y: rect.top + 5,
        }
    }

    #[test]
    fn must_pass_deepest_hit_hwnd_follows_real_z_order() {
        let parent = parent();
        // A large docked panel and a small canvas child that it fully covers.
        let panel = parent.child(0, 0, 200);
        let canvas = parent.child(50, 50, 40);
        parent.place(canvas, HWND_BOTTOM);
        parent.place(panel, HWND_TOP);
        let point = inside(canvas);

        // Both contain the point; the topmost one wins, not the smaller one.
        assert_eq!(deepest_hit_hwnd(parent.hwnd(), point), panel);

        // Same windows, z-order swapped: the walk must follow the real order.
        parent.place(panel, HWND_BOTTOM);
        assert_eq!(deepest_hit_hwnd(parent.hwnd(), point), canvas);
    }
}
