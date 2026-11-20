//! FROZEN behaviour table of Gitea #2332, rows B1-B8 (added 2026-11-19 after the
//! real-app green run found the point box placed with screen coordinates on a
//! window-local capture). Written by the manager before the fix. Coders never
//! edit this file; a wrong row is reported to the manager.
//!
//! The box around a click point is 1024 x 384 pixels (512 left and right, 192
//! above and below), in screen coordinates. `in_capture` gives the part of it
//! inside a capture whose pixel (0, 0) sits at `capture_origin` on the screen,
//! in the capture's own coordinates.

use synapse_core::{Point, Rect};

use super::PointBox;

const fn point(x: i32, y: i32) -> Point {
    Point { x, y }
}

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Option<Rect> {
    Some(Rect { x, y, w, h })
}

#[test]
fn point_box_table_2332b() {
    // (row, click point on screen, capture origin on screen, capture w, capture h, expected)
    let rows: [(&str, Point, Point, u32, u32, Option<Rect>); 10] = [
        (
            "B1_window_at_the_screen_corner",
            point(99, 100),
            point(0, 0),
            422,
            400,
            rect(0, 0, 422, 292),
        ),
        (
            "B2_window_at_520_60_same_click_inside_the_window",
            point(619, 163),
            point(520, 60),
            422,
            400,
            rect(0, 0, 422, 295),
        ),
        (
            "B3_window_at_980_40_was_unavailable_on_the_candidate",
            point(1079, 143),
            point(980, 40),
            422,
            400,
            rect(0, 0, 422, 295),
        ),
        (
            "B4_wide_window_box_fits_inside",
            point(1600, 700),
            point(100, 100),
            2100,
            1200,
            rect(988, 408, 1024, 384),
        ),
        (
            "B5_click_near_the_bottom_right_corner_is_cut_at_the_capture",
            point(1090, 1090),
            point(100, 100),
            1000,
            1000,
            rect(478, 798, 522, 202),
        ),
        (
            "B6_window_on_a_monitor_left_of_the_main_one",
            point(-900, 150),
            point(-1000, 50),
            422,
            400,
            rect(0, 0, 422, 292),
        ),
        (
            "B7_box_entirely_left_of_the_capture",
            point(100, 300),
            point(700, 100),
            422,
            400,
            None,
        ),
        (
            "B7_box_entirely_below_the_capture",
            point(900, 900),
            point(700, 100),
            422,
            400,
            None,
        ),
        ("B8_empty_capture", point(100, 100), point(0, 0), 0, 0, None),
        (
            "B8_box_touches_the_capture_edge_only",
            point(1212, 300),
            point(100, 100),
            600,
            400,
            None,
        ),
    ];

    let mut failures = Vec::new();
    for (row, click, origin, width, height, expected) in &rows {
        let observed = PointBox::around(*click).in_capture(*origin, *width, *height);
        if observed != *expected {
            failures.push(format!(
                "{row}: expected {expected:?}, observed {observed:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
