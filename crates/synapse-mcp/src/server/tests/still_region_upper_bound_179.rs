//! #179: native still regions must be bounded before capture/allocation.
use super::validate_screenshot_region;
use synapse_core::Rect;

#[test]
fn must_pass_still_region_includes_edge_and_pixel_budget_boundaries() {
    for (w, h) in [(1, 1), (16_384, 1), (1, 16_384), (10_000, 10_000)] {
        assert_eq!(
            validate_screenshot_region(Rect {
                x: -20,
                y: 30,
                w,
                h
            })
            .expect("within limit"),
            ()
        );
    }
}

#[test]
fn must_fail_still_region_over_edge_or_area_is_capture_target_invalid() {
    for (w, h) in [
        (16_385, 1),
        (1, 16_385),
        (10_000, 10_001),
        (i32::MAX, i32::MAX),
    ] {
        let region = Rect { x: 0, y: 0, w, h };
        let error = validate_screenshot_region(region).expect_err("oversize region");
        assert_eq!(
            error.data.as_ref().unwrap()["code"],
            "CAPTURE_TARGET_INVALID"
        );
        assert!(error.message.contains("exceeds"), "{error}");
        assert_eq!((region.w, region.h), (w, h));
    }
}

#[test]
fn must_fail_empty_region_preserves_nonempty_diagnostic() {
    for (w, h) in [(0, 1), (1, 0), (-1, 1)] {
        let error =
            validate_screenshot_region(Rect { x: 0, y: 0, w, h }).expect_err("empty region");
        assert_eq!(
            error.data.as_ref().unwrap()["code"],
            "CAPTURE_TARGET_INVALID"
        );
        assert!(error.message.contains("region must be non-empty"));
    }
}
