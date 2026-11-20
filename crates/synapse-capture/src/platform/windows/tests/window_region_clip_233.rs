//! Regression tests for #233: a maximized window's client area overhangs the
//! monitor by a pixel, so its client origin maps to frame-bitmap (-1, -1). A
//! valid client region that overlaps the bitmap is clipped to the bitmap, and
//! the clipped pixels are reported in both bitmap and client coordinates; only
//! a region with no overlap is refused.

use synapse_core::Rect;

use super::client_region_to_frame_region;
use crate::{CaptureError, ClientWindowRegion};

/// The live Spotify case: 1922x1022 client at frame offset (-1, -1) inside a
/// 1920x1021 capture bitmap.
fn map_maximized(region: Rect) -> Result<ClientWindowRegion, CaptureError> {
    client_region_to_frame_region(region, 1922, 1022, -1, -1, 1920, 1021)
}

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn must_pass_maximized_client_origin_overhang_is_clipped_to_the_bitmap() {
    let mapped = map_maximized(rect(0, 0, 600, 120))
        .unwrap_or_else(|error| panic!("overlapping region must be clipped, not refused: {error}"));
    assert_eq!(mapped.window, rect(0, 0, 599, 119));
    assert_eq!(mapped.client, rect(1, 1, 599, 119));
}

#[test]
fn must_pass_region_overhanging_the_right_edge_is_clipped() {
    let mapped = map_maximized(rect(1900, 1000, 22, 22))
        .unwrap_or_else(|error| panic!("overlapping region must be clipped, not refused: {error}"));
    assert_eq!(mapped.window, rect(1899, 999, 21, 22));
    assert_eq!(mapped.client, rect(1900, 1000, 21, 22));
}

#[test]
fn must_pass_region_fully_inside_the_bitmap_is_unchanged() {
    let mapped = map_maximized(rect(10, 20, 30, 40))
        .unwrap_or_else(|error| panic!("inside region must map: {error}"));
    assert_eq!(mapped.window, rect(9, 19, 30, 40));
    assert_eq!(mapped.client, rect(10, 20, 30, 40));
}

#[test]
fn must_fail_region_only_inside_the_overhang_is_rejected() {
    let error = map_maximized(rect(0, 0, 1, 1))
        .expect_err("a region with no pixel inside the bitmap must be refused");
    assert!(
        matches!(error, CaptureError::TargetInvalid { .. }),
        "{error:?}"
    );
}

#[test]
fn must_pass_region_partly_past_the_client_area_is_clipped_to_the_client_rect() {
    // Manager decision on #233: a partial overhang of the client area is
    // clipped (not refused) and the clipped rectangle is reported.
    let mapped = map_maximized(rect(1900, 0, 23, 10))
        .unwrap_or_else(|error| panic!("partial client overhang must be clipped: {error}"));
    assert_eq!(mapped.client, rect(1900, 1, 21, 9));
    assert_eq!(mapped.window, rect(1899, 0, 21, 9));
}

#[test]
fn must_pass_negative_origin_region_is_clipped_to_the_client_origin() {
    let mapped = client_region_to_frame_region(rect(-20, -15, 320, 150), 300, 160, 0, 0, 300, 160)
        .unwrap_or_else(|error| panic!("negative-origin overlap must be clipped: {error}"));
    assert_eq!(mapped.client, rect(0, 0, 300, 135));
    assert_eq!(mapped.window, rect(0, 0, 300, 135));
}

#[test]
fn must_fail_region_with_zero_client_overlap_is_refused() {
    for region in [
        rect(1922, 0, 10, 10),
        rect(0, 1022, 10, 10),
        rect(-30, -30, 30, 30),
        rect(-10, 5, 10, 10),
    ] {
        let error =
            map_maximized(region).expect_err("a region with no client pixel must be refused");
        assert!(
            matches!(error, CaptureError::TargetInvalid { .. }),
            "{region:?}: {error:?}"
        );
        assert!(
            error.to_string().contains("client area"),
            "{region:?}: {error}"
        );
    }
}
