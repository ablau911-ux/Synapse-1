//! #191: `act_scroll at=` with `verify_delta` captures pixel evidence from the
//! window under the point, not from whatever window has the foreground. The
//! window is resolved once per scroll and both captures use it, so a window
//! change between the before and after capture cannot split the evidence.

use std::cell::Cell;

use super::m2_tools_source_scan::method_body;
use super::{DeltaRegion, Point, point_region_target_window_with};

/// Fake desktop: x < 50 is window 0xA, x >= 50 is window 0xB.
fn root_at(point: Point) -> Option<i64> {
    Some(if point.x < 50 { 0xA } else { 0xB })
}

#[test]
fn must_pass_a_point_scroll_captures_the_window_under_the_point() {
    assert_eq!(
        point_region_target_window_with(Some(Point { x: 80, y: 10 }), root_at),
        Some(0xB)
    );
}

#[test]
fn must_fail_no_point_never_invents_a_target_window() {
    assert_eq!(point_region_target_window_with(None, root_at), None);
}

#[test]
fn must_fail_a_point_over_no_window_falls_back_to_the_foreground() {
    assert_eq!(
        point_region_target_window_with(Some(Point { x: 80, y: 10 }), |_| None),
        None
    );
}

#[test]
fn must_fail_the_after_capture_never_re_resolves_the_window() {
    let calls = Cell::new(0_u32);
    let region = DeltaRegion::resolve(Some(Point { x: 80, y: 10 }), |point| {
        calls.set(calls.get() + 1);
        root_at(point)
    });
    assert_eq!(region.target_window, Some(0xB));
    assert_eq!(
        calls.get(),
        1,
        "the window under the point is resolved once"
    );

    // The real call sites (#62 moved the one hit test into the claim check):
    // act_scroll resolves the point once through `scroll_claim_target` and
    // builds the region from that carried window with no hit test of its own;
    // the after-capture in verify_action_delta never resolves again.
    let act_scroll = method_body("act_scroll");
    assert_eq!(act_scroll.matches("scroll_claim_target(").count(), 1);
    assert_eq!(
        act_scroll
            .matches(
                "DeltaRegion::resolve(params.verify_delta_point_region(), |_| checked_window_hwnd)"
            )
            .count(),
        1,
        "the capture region carries the window the claim check resolved"
    );
    for resolver in [
        "DeltaRegion::at_point(",
        "point_region_target_window(",
        "root_hwnd_at_point",
    ] {
        assert!(
            !act_scroll.contains(resolver),
            "act_scroll re-resolves via {resolver}"
        );
    }
    let verify = method_body("verify_action_delta");
    for resolver in [
        "DeltaRegion::at_point(",
        "point_region_target_window(",
        "root_hwnd_at_point",
    ] {
        assert!(
            !verify.contains(resolver),
            "after-capture re-resolves via {resolver}"
        );
    }
    assert!(verify.contains("region.target_window"));
}

#[test]
fn must_pass_no_point_region_has_no_target_window() {
    let region = DeltaRegion::resolve(None, |_| panic!("no point, no resolve"));
    assert_eq!(region.point, None);
    assert_eq!(region.target_window, None);
}
