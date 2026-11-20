use super::*;
use crate::m2::native_race_windows_62::Windows;
use windows::Win32::UI::WindowsAndMessaging::{WM_MOUSEHWHEEL, WM_MOUSEWHEEL};

#[test]
fn must_pass_bound_child_gets_exact_vertical_and_horizontal_wheel_payloads() {
    let windows = Windows::new();
    let point = Point { x: 17, y: 29 };
    assert!(windows.drain(WM_MOUSEWHEEL).is_empty());
    assert!(windows.drain(WM_MOUSEHWHEEL).is_empty());
    post_wheel_message(
        windows.child_id(),
        WM_MOUSEWHEEL,
        -120,
        point,
        windows.root_id(),
    )
    .expect("vertical");
    post_wheel_message(
        windows.child_id(),
        WM_MOUSEHWHEEL,
        120,
        point,
        windows.root_id(),
    )
    .expect("horizontal");
    assert_eq!(
        windows.drain(WM_MOUSEWHEEL),
        vec![(0xff88_0000, (29 << 16) | 17)]
    );
    assert_eq!(
        windows.drain(WM_MOUSEHWHEEL),
        vec![(0x0078_0000, (29 << 16) | 17)]
    );
}

#[test]
fn must_fail_reparented_child_receives_no_vertical_or_horizontal_wheel() {
    let windows = Windows::new();
    let checked = windows.root_id();
    assert_eq!(windows.root_of_child(), checked);
    windows.reparent();
    for message in [WM_MOUSEWHEEL, WM_MOUSEHWHEEL] {
        let error = post_wheel_message(
            windows.child_id(),
            message,
            120,
            Point { x: 17, y: 29 },
            checked,
        )
        .expect_err("native root changed after check");
        assert!(matches!(error, ActionError::TargetInvalid { .. }));
        assert!(
            windows.drain(message).is_empty(),
            "refusal must leave destination queue untouched"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_no_checked_target_has_zero_wheel_posts() {
    let windows = Windows::new();
    let error = execute_targeted_scroll_actions_platform(
        &ActScrollParams {
            dy: -1,
            ..Default::default()
        },
        Point { x: 17, y: 29 },
        None,
    )
    .await
    .expect_err("checked target required");
    assert_eq!(
        error.data.as_ref().expect("data")["code"],
        "ACTION_TARGET_INVALID"
    );
    assert!(windows.drain(WM_MOUSEWHEEL).is_empty());
    assert!(windows.drain(WM_MOUSEHWHEEL).is_empty());
}

// NEEDS-SEAM: production execute_targeted_scroll_actions_platform must delegate to:
// async fn execute_targeted_scroll_actions_platform_with(
//   params: &ActScrollParams, point: Point, checked_window_hwnd: Option<i64>,
//   before_post: impl FnMut(usize),
// ) -> Result<ScrollDispatchResult, ErrorData>
// Callback runs after real native point/readback resolution, immediately before each post.
// The native fixture below presents only its own nonactivating window.
#[tokio::test(flavor = "current_thread")]
// The desktop-topology guard serializes this whole test body on purpose (its
// window is moved under a screen point); the current_thread runtime has no
// other task that could contend for it.
#[allow(clippy::await_holding_lock)]
async fn must_fail_reparent_between_point_readback_and_first_post_sends_no_wheel() {
    let _serial = crate::test_support::desktop_topology_serial();
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos,
    };
    let windows = Windows::new();
    // SAFETY: position/show only our test window, without activation.
    unsafe {
        SetWindowPos(
            windows.root,
            Some(HWND_TOPMOST),
            100,
            100,
            100,
            100,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .expect("show own fixture");
    let point = Point { x: 120, y: 120 };
    assert_eq!(root_hwnd_at_point(point), Some(windows.root_id()));
    let mut calls = 0;
    let result = execute_targeted_scroll_actions_platform_with(
        &ActScrollParams {
            dy: -1,
            dx: 1,
            ..Default::default()
        },
        point,
        Some(windows.root_id()),
        |index| {
            assert_eq!(index, 0);
            calls += 1;
            windows.reparent();
        },
    )
    .await;
    assert_eq!(calls, 1, "race hook must run at real delivery boundary");
    let error = result.expect_err("child root changed after point readback");
    assert_eq!(
        error.data.as_ref().expect("data")["code"],
        "ACTION_TARGET_INVALID"
    );
    assert!(windows.drain(WM_MOUSEWHEEL).is_empty());
    assert!(windows.drain(WM_MOUSEHWHEEL).is_empty());
}

#[test]
fn must_fail_point_now_over_another_root_cannot_use_previously_checked_window() {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos,
    };
    // Other tests' topmost windows must not cover the point while it is read.
    let _serial = crate::test_support::desktop_topology_serial();
    let windows = Windows::new();
    let point = Point { x: 120, y: 120 };
    // SAFETY: only test-owned no-activation roots are positioned.
    unsafe {
        SetWindowPos(
            windows.root,
            Some(HWND_TOPMOST),
            100,
            100,
            100,
            100,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .expect("initial point target");
    assert_eq!(root_hwnd_at_point(point), Some(windows.root_id()));
    unsafe {
        SetWindowPos(
            windows.other,
            Some(HWND_TOPMOST),
            100,
            100,
            100,
            100,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .expect("changed point target");
    assert_eq!(
        root_hwnd_at_point(point),
        Some(windows.other.0 as isize as i64)
    );
    let error = windows_hwnd_message_scroll_readback(point, windows.root_id())
        .expect_err("point must remain under the checked root");
    assert!(matches!(error, ActionError::TargetInvalid { .. }));
    assert!(windows.drain(WM_MOUSEWHEEL).is_empty());
    assert!(windows.drain(WM_MOUSEHWHEEL).is_empty());
}
