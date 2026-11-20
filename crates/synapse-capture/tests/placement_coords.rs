//! Regression tests for the `WINDOWPLACEMENT::rcNormalPosition`
//! workspace-to-screen conversion (#130 review): minimized bounds for
//! captioned windows are workspace-relative, so a taskbar docked to the top or
//! left shifts the raw rect away from screen coordinates.

use synapse_capture::workspace_rect_to_screen;
use synapse_core::Rect;

#[test]
fn must_pass_workspace_origin_offsets_screen_rect() {
    let placement = Rect {
        x: 100,
        y: 50,
        w: 800,
        h: 600,
    };
    // Taskbar docked to the top: the work area starts below it.
    let top_docked = Rect {
        x: 0,
        y: 48,
        w: 1920,
        h: 1032,
    };
    assert_eq!(
        workspace_rect_to_screen(placement, top_docked),
        Rect {
            x: 100,
            y: 98,
            w: 800,
            h: 600
        }
    );
    // Taskbar docked to the left: the work area starts right of it.
    let left_docked = Rect {
        x: 64,
        y: 0,
        w: 1856,
        h: 1080,
    };
    assert_eq!(
        workspace_rect_to_screen(placement, left_docked),
        Rect {
            x: 164,
            y: 50,
            w: 800,
            h: 600
        }
    );
}

#[test]
fn must_pass_fullscreen_work_area_leaves_rect_unchanged() {
    let placement = Rect {
        x: 100,
        y: 50,
        w: 800,
        h: 600,
    };
    let fullscreen = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    assert_eq!(workspace_rect_to_screen(placement, fullscreen), placement);
}

#[test]
fn must_fail_extreme_offsets_saturate_instead_of_wrapping() {
    let far_positive = Rect {
        x: i32::MAX - 10,
        y: i32::MAX,
        w: 800,
        h: 600,
    };
    let positive_area = Rect {
        x: 64,
        y: 48,
        w: 1856,
        h: 1032,
    };
    assert_eq!(
        workspace_rect_to_screen(far_positive, positive_area),
        Rect {
            x: i32::MAX,
            y: i32::MAX,
            w: 800,
            h: 600
        }
    );

    let far_negative = Rect {
        x: i32::MIN + 10,
        y: i32::MIN,
        w: 800,
        h: 600,
    };
    let negative_area = Rect {
        x: -1920,
        y: -1080,
        w: 1920,
        h: 1080,
    };
    assert_eq!(
        workspace_rect_to_screen(far_negative, negative_area),
        Rect {
            x: i32::MIN,
            y: i32::MIN,
            w: 800,
            h: 600
        }
    );
}

#[test]
fn must_fail_work_area_size_never_leaks_into_window_size() {
    let placement = Rect {
        x: 10,
        y: 20,
        w: 0,
        h: -5,
    };
    let odd_area = Rect {
        x: -1920,
        y: 0,
        w: i32::MAX,
        h: i32::MIN,
    };
    assert_eq!(
        workspace_rect_to_screen(placement, odd_area),
        Rect {
            x: -1910,
            y: 20,
            w: 0,
            h: -5
        }
    );
}
