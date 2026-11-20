//! #77 / #101: the snapshot must say when it did not see everything and must
//! not present scrolled-out controls as visible geometry.

use synapse_core::Rect;

use super::{bbox_for_visibility, depth_cut_hides_children};

const RECT: Rect = Rect {
    x: 10,
    y: 20,
    w: 100,
    h: 50,
};

#[test]
fn must_fail_offscreen_control_has_no_visible_extent_but_keeps_its_origin() {
    let rect = bbox_for_visibility(RECT, true);
    assert_eq!((rect.w, rect.h), (0, 0));
    assert_eq!((rect.x, rect.y), (RECT.x, RECT.y));
}

#[test]
fn must_pass_onscreen_control_keeps_its_rectangle() {
    assert_eq!(bbox_for_visibility(RECT, false), RECT);
}

#[test]
fn must_fail_children_beyond_the_requested_depth_mark_the_snapshot_incomplete() {
    assert!(depth_cut_hides_children(2, 2, true));
    assert!(depth_cut_hides_children(3, 2, true));
}

#[test]
fn must_pass_leaves_at_the_depth_limit_and_shallower_nodes_do_not() {
    assert!(!depth_cut_hides_children(2, 2, false));
    assert!(!depth_cut_hides_children(1, 2, true));
    assert!(!depth_cut_hides_children(0, 2, false));
}

// #101 CDP path: `DOM.getBoxModel` quads are viewport-relative, so a web node
// with no intersection with the viewport is scrolled out and must lose its
// visible extent exactly like a UIA offscreen control.

use crate::bbox_in_viewport;

fn web(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn must_fail_web_node_below_the_viewport_has_no_visible_extent() {
    let rect = bbox_in_viewport(web(20, 900, 100, 30), 1280, 720);
    assert_eq!((rect.w, rect.h), (0, 0));
    assert_eq!((rect.x, rect.y), (20, 900));
}

#[test]
fn must_fail_web_node_above_left_and_right_of_the_viewport_have_no_visible_extent() {
    for rect in [
        web(20, -80, 100, 30),
        web(-300, 10, 100, 30),
        web(1280, 10, 100, 30),
    ] {
        let hidden = bbox_in_viewport(rect, 1280, 720);
        assert_eq!((hidden.w, hidden.h), (0, 0), "{rect:?}");
    }
}

#[test]
fn must_fail_web_node_touching_the_viewport_edge_without_overlap_is_hidden() {
    assert_eq!(bbox_in_viewport(web(0, 720, 50, 20), 1280, 720).h, 0);
    assert_eq!(bbox_in_viewport(web(-50, 0, 50, 20), 1280, 720).w, 0);
}

#[test]
fn must_pass_web_node_inside_or_partly_inside_the_viewport_keeps_its_rectangle() {
    let inside = web(20, 40, 100, 30);
    let partly = web(1250, 700, 100, 80);
    assert_eq!(bbox_in_viewport(inside, 1280, 720), inside);
    assert_eq!(bbox_in_viewport(partly, 1280, 720), partly);
}
