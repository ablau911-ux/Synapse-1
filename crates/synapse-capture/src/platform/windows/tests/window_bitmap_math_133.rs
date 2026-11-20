//! Regression tests for the whole-window / client-region bitmap math (#133).
//!
//! The PrintWindow bitmap is sized by the DWM extended frame bounds, which is
//! smaller than the `GetWindowRect` rect (invisible resize border/shadow). These
//! tests pin the pure core: client region -> frame-bitmap region, the
//! inside-bitmap validation, and the pixel crop, at DPI scales 1.25 / 1.5 / 2.0
//! and for a hidden-desktop style window whose bitmap extent differs from
//! `GetWindowRect`. Everything here is in physical pixels, as the worker process
//! is per-monitor-v2 DPI aware.

use synapse_core::Rect;

use super::{
    client_region_to_frame_region, copy_bgra_region_from_bytes, validate_region_inside_window,
};
use crate::CaptureError;

/// One DPI-scaled window: client extent, client origin inside the frame bitmap,
/// and the DWM frame (== PrintWindow bitmap) extent.
struct ScaledWindow {
    label: &'static str,
    client_w: i32,
    client_h: i32,
    offset_x: i32,
    offset_y: i32,
    frame_w: i32,
    frame_h: i32,
}

/// Base 800x480 client with an (8, 32) border/caption offset at 100% DPI; the
/// frame adds the offset on the left/right and bottom.
fn scaled_window(label: &'static str, scale_percent: i32) -> ScaledWindow {
    let scale = |value: i32| value * scale_percent / 100;
    let client_w = scale(800);
    let client_h = scale(480);
    let offset_x = scale(8);
    let offset_y = scale(32);
    ScaledWindow {
        label,
        client_w,
        client_h,
        offset_x,
        offset_y,
        frame_w: client_w + 2 * offset_x,
        frame_h: client_h + offset_y + offset_x,
    }
}

fn dpi_cases() -> [ScaledWindow; 3] {
    [
        scaled_window("125%", 125),
        scaled_window("150%", 150),
        scaled_window("200%", 200),
    ]
}

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

fn map_client(window: &ScaledWindow, region: Rect) -> Result<Rect, CaptureError> {
    map_client_regions(window, region).map(|resolved| resolved.window)
}

fn map_client_regions(
    window: &ScaledWindow,
    region: Rect,
) -> Result<crate::ClientWindowRegion, CaptureError> {
    client_region_to_frame_region(
        region,
        window.client_w,
        window.client_h,
        window.offset_x,
        window.offset_y,
        window.frame_w,
        window.frame_h,
    )
}

#[test]
fn must_pass_full_client_region_maps_inside_frame_bitmap_at_every_dpi_scale() {
    for window in dpi_cases() {
        let mapped = map_client(
            &window,
            Rect {
                x: 0,
                y: 0,
                w: window.client_w,
                h: window.client_h,
            },
        )
        .unwrap_or_else(|error| panic!("{}: full client area must fit: {error}", window.label));

        assert_eq!(
            mapped,
            Rect {
                x: window.offset_x,
                y: window.offset_y,
                w: window.client_w,
                h: window.client_h,
            },
            "{}",
            window.label
        );
        assert!(
            mapped.x + mapped.w <= window.frame_w && mapped.y + mapped.h <= window.frame_h,
            "{}: mapped region {mapped:?} escapes the {}x{} frame bitmap",
            window.label,
            window.frame_w,
            window.frame_h
        );
    }
}

#[test]
fn must_pass_interior_client_region_keeps_size_and_shifts_by_client_origin() {
    for window in dpi_cases() {
        let region = Rect {
            x: 17,
            y: 23,
            w: 211,
            h: 97,
        };
        let mapped = map_client(&window, region)
            .unwrap_or_else(|error| panic!("{}: interior region must fit: {error}", window.label));

        assert_eq!(
            mapped,
            Rect {
                x: 17 + window.offset_x,
                y: 23 + window.offset_y,
                w: 211,
                h: 97,
            },
            "{}",
            window.label
        );
    }
}

#[test]
fn must_pass_client_region_one_pixel_past_client_area_is_clipped_at_every_dpi_scale() {
    // #233 manager decision: a partial client-area overhang is clipped to the
    // client rect, and the clipped rect is reported in both coordinate spaces.
    for window in dpi_cases() {
        let cases = [
            (
                rect(0, 0, window.client_w + 1, window.client_h),
                rect(0, 0, window.client_w, window.client_h),
            ),
            (
                rect(0, 0, window.client_w, window.client_h + 1),
                rect(0, 0, window.client_w, window.client_h),
            ),
            (rect(-1, 0, 10, 10), rect(0, 0, 9, 10)),
        ];
        for (region, clipped) in cases {
            let resolved = map_client_regions(&window, region)
                .unwrap_or_else(|error| panic!("{}: {region:?} must clip: {error}", window.label));
            assert_eq!(resolved.client, clipped, "{}", window.label);
            let shifted = rect(
                clipped.x + window.offset_x,
                clipped.y + window.offset_y,
                clipped.w,
                clipped.h,
            );
            assert_eq!(resolved.window, shifted, "{}", window.label);
        }
    }
}

#[test]
fn must_fail_client_region_with_zero_client_overlap_is_rejected_at_every_dpi_scale() {
    for window in dpi_cases() {
        for region in [
            rect(window.client_w, 0, 10, 10),
            rect(0, window.client_h, 10, 10),
            rect(-10, -10, 10, 10),
        ] {
            let error = map_client(&window, region)
                .expect_err("region with no client pixel must be rejected");
            assert!(
                matches!(error, CaptureError::TargetInvalid { .. }),
                "{}: {error:?}",
                window.label
            );
            assert!(
                error.to_string().contains("client area"),
                "{}: {error}",
                window.label
            );
        }
    }
}

#[test]
fn must_fail_getwindowrect_sized_region_exceeds_dwm_frame_bitmap_but_frame_sized_fits() {
    // Hidden-desktop style window (the #133 report): DWM frame / PrintWindow
    // bitmap is 1502x1039 while GetWindowRect-derived bounds are 1518x1047
    // (invisible margin on the left/right/bottom). The invisible margin scales
    // with DPI (base 8 px).
    let frame_w = 1502;
    let frame_h = 1039;
    for (label, scale_percent) in [("100%", 100), ("125%", 125), ("150%", 150), ("200%", 200)] {
        let margin = 8 * scale_percent / 100;
        let rect_w = frame_w + 2 * margin;
        let rect_h = frame_h + margin;

        let from_window_rect = validate_region_inside_window(
            Rect {
                x: 0,
                y: 0,
                w: rect_w,
                h: rect_h,
            },
            frame_w,
            frame_h,
        )
        .expect_err("a GetWindowRect-sized region does not fit the DWM-sized bitmap");
        assert!(
            matches!(from_window_rect, CaptureError::TargetInvalid { .. }),
            "{label}: {from_window_rect:?}"
        );

        validate_region_inside_window(
            Rect {
                x: 0,
                y: 0,
                w: frame_w,
                h: frame_h,
            },
            frame_w,
            frame_h,
        )
        .unwrap_or_else(|error| panic!("{label}: frame-sized region must fit: {error}"));
    }
}

/// BGRA bitmap whose pixel (x, y) encodes its own column in B/G and row in R/A,
/// so a crop can be asserted against exact source pixels.
fn coordinate_bitmap(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&[
                (x & 0xff) as u8,
                (x >> 8) as u8,
                (y & 0xff) as u8,
                (y >> 8) as u8,
            ]);
        }
    }
    bytes
}

fn pixel_xy(bytes: &[u8], width: u32, x: u32, y: u32) -> (u32, u32) {
    let at = (y as usize * width as usize + x as usize) * 4;
    (
        u32::from(bytes[at]) | (u32::from(bytes[at + 1]) << 8),
        u32::from(bytes[at + 2]) | (u32::from(bytes[at + 3]) << 8),
    )
}

#[test]
fn must_pass_mapped_client_region_crops_the_expected_source_pixels_at_every_dpi_scale() {
    for window in dpi_cases() {
        let width = u32::try_from(window.frame_w).expect("positive frame width");
        let height = u32::try_from(window.frame_h).expect("positive frame height");
        let full = coordinate_bitmap(width, height);
        let client_region = Rect {
            x: 5,
            y: 9,
            w: 64,
            h: 32,
        };
        let mapped = map_client(&window, client_region).expect("interior region fits");

        let cropped = copy_bgra_region_from_bytes(&full, width, mapped)
            .unwrap_or_else(|error| panic!("{}: crop must succeed: {error}", window.label));

        assert_eq!(cropped.len(), 64 * 32 * 4, "{}", window.label);
        let crop_w = 64_u32;
        let origin_x = u32::try_from(mapped.x).expect("positive x");
        let origin_y = u32::try_from(mapped.y).expect("positive y");
        for (cx, cy) in [(0, 0), (63, 0), (0, 31), (63, 31), (17, 11)] {
            assert_eq!(
                pixel_xy(&cropped, crop_w, cx, cy),
                (origin_x + cx, origin_y + cy),
                "{}: crop pixel ({cx}, {cy})",
                window.label
            );
        }
    }
}
