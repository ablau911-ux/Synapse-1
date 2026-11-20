//! #194: a Chromium foreground fallback must never click a bbox centre that lies
//! outside the target window (stale or below-the-fold UIA bbox).

use super::foreground_click_actionability::visible_click_point;
use synapse_core::{Point, Rect, error_codes};

const WINDOW: Rect = Rect {
    x: 100,
    y: 100,
    w: 800,
    h: 600,
};

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn must_pass_bbox_inside_the_window_clicks_its_centre() {
    let point = visible_click_point(rect(200, 200, 100, 40), WINDOW).expect("inside window");
    assert_eq!(point, Point { x: 250, y: 220 });
}

#[test]
fn must_fail_bbox_below_the_viewport_is_refused_not_clicked() {
    let refusal = visible_click_point(rect(200, 1500, 100, 40), WINDOW).expect_err("below window");
    assert_eq!(refusal.code, error_codes::ACTION_TARGET_INVALID);
    assert_eq!(refusal.reason, "target_click_point_outside_window");
    assert!(refusal.detail.contains("(250, 1520)"), "{}", refusal.detail);
}

#[test]
fn must_fail_bbox_above_left_and_right_of_the_window_is_refused() {
    for bbox in [
        rect(200, -300, 100, 40),
        rect(-500, 200, 100, 40),
        rect(2000, 200, 100, 40),
    ] {
        let refusal = visible_click_point(bbox, WINDOW).expect_err("outside window");
        assert_eq!(
            refusal.reason, "target_click_point_outside_window",
            "{bbox:?}"
        );
    }
}

#[test]
fn must_fail_window_edges_are_top_left_inclusive_bottom_right_exclusive() {
    assert!(visible_click_point(rect(100, 100, 0, 0), WINDOW).is_err());
    assert_eq!(
        visible_click_point(rect(99, 99, 2, 2), WINDOW).expect("centre on top-left pixel"),
        Point { x: 100, y: 100 }
    );
    let refusal = visible_click_point(rect(898, 698, 4, 4), WINDOW).expect_err("centre on 900,700");
    assert_eq!(refusal.reason, "target_click_point_outside_window");
}

#[test]
fn must_fail_empty_bbox_and_empty_window_are_refused() {
    let empty_bbox = visible_click_point(rect(200, 200, 0, 40), WINDOW).expect_err("empty bbox");
    assert_eq!(empty_bbox.reason, "target_bbox_empty");
    let empty_window =
        visible_click_point(rect(200, 200, 100, 40), rect(0, 0, 0, 0)).expect_err("empty window");
    assert_eq!(empty_window.reason, "target_click_point_outside_window");
}

#[test]
fn must_fail_centre_overflow_is_refused() {
    let refusal = visible_click_point(rect(i32::MAX - 1, 10, i32::MAX, 10), WINDOW)
        .expect_err("centre past i32::MAX");
    assert_eq!(refusal.reason, "target_bbox_center_overflow");
}
