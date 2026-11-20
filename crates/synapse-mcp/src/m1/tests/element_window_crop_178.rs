//! Crop math for `read_text(element_id)` window capture (#178).

use synapse_core::Rect;

use super::crop_element_to_window_frame;

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn must_pass_element_inside_window_maps_to_frame_relative_origin() {
    let crop = crop_element_to_window_frame(rect(300, 250, 100, 40), rect(200, 200, 800, 600))
        .expect("element inside window");
    assert_eq!(crop.screen_region, rect(300, 250, 100, 40));
    assert_eq!(crop.window_region, rect(100, 50, 100, 40));
}

#[test]
fn must_pass_element_partly_outside_window_is_clipped_to_the_window() {
    let crop = crop_element_to_window_frame(rect(950, 780, 100, 100), rect(200, 200, 800, 600))
        .expect("overlapping element");
    assert_eq!(crop.screen_region, rect(950, 780, 50, 20));
    assert_eq!(crop.window_region, rect(750, 580, 50, 20));
}

#[test]
fn must_pass_negative_screen_origin_window_is_supported() {
    let crop = crop_element_to_window_frame(rect(-1800, 100, 200, 30), rect(-1920, 0, 1000, 700))
        .expect("secondary monitor window");
    assert_eq!(crop.window_region, rect(120, 100, 200, 30));
}

#[test]
fn must_fail_element_fully_outside_or_touching_edge_has_no_crop() {
    let frame = rect(200, 200, 800, 600);
    assert_eq!(
        crop_element_to_window_frame(rect(1000, 300, 50, 50), frame),
        None
    );
    assert_eq!(
        crop_element_to_window_frame(rect(0, 0, 50, 50), frame),
        None
    );
    assert_eq!(
        crop_element_to_window_frame(rect(300, 800, 50, 50), frame),
        None
    );
}

#[test]
fn must_fail_empty_element_or_empty_frame_has_no_crop() {
    let frame = rect(200, 200, 800, 600);
    assert_eq!(
        crop_element_to_window_frame(rect(300, 300, 0, 10), frame),
        None
    );
    assert_eq!(
        crop_element_to_window_frame(rect(300, 300, 10, -5), frame),
        None
    );
    assert_eq!(
        crop_element_to_window_frame(rect(300, 300, 10, 10), rect(200, 200, 0, 600)),
        None
    );
}

#[test]
fn must_fail_extreme_coordinates_do_not_overflow() {
    let crop = crop_element_to_window_frame(
        rect(i32::MAX - 5, 0, i32::MAX, 10),
        rect(0, 0, i32::MAX, 100),
    )
    .expect("overlap of 5 px at the far edge");
    assert_eq!(crop.window_region.w, 5);
    assert_eq!(
        crop_element_to_window_frame(rect(i32::MIN, 0, 10, 10), rect(0, 0, 10, 10)),
        None
    );
}
