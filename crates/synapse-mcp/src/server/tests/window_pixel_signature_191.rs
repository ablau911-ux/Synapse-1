//! #191 regression: `verify_delta` pixel evidence must come from the TARGET
//! window's own capture (WGC frame), never from a screen-DC region at the
//! window's bounds. A screen capture shows whatever window is on top, so an
//! occluded or background target hashed another window (false success when the
//! covering window changes, false `ACTION_NO_OBSERVED_DELTA` when the covered
//! target really changed).
//!
//! The pure conversion `window_pixel_evidence_from_capture` takes ONLY the
//! window capture result, so there is no screen fallback to hash another
//! window: a failed window capture yields unavailable pixel evidence.

use sha2::{Digest as _, Sha256};
use synapse_capture::{CaptureError, CapturedBgraBitmap, CapturedWindowBgraBitmap};
use synapse_core::{Point, Rect};

use super::pixel_signature::window_pixel_evidence_from_capture;
use super::*;

const FRAME_W: u32 = 1600;
const FRAME_H: u32 = 800;

/// The real WGC window frame is window-local (#2332): its own region starts at
/// (0, 0) and its screen position (here 100, 50) is read separately.
fn frame_region() -> Rect {
    Rect {
        x: 0,
        y: 0,
        w: 1600,
        h: 800,
    }
}

fn frame_origin() -> Option<Point> {
    Some(Point { x: 100, y: 50 })
}

fn frame(fill: u8) -> CapturedWindowBgraBitmap {
    CapturedWindowBgraBitmap {
        bitmap: CapturedBgraBitmap {
            region: frame_region(),
            width: FRAME_W,
            height: FRAME_H,
            bytes: vec![fill; FRAME_W as usize * FRAME_H as usize * 4],
        },
        capture_backend: "graphics_capture_window_bgra",
        capture_attempts: 1,
        capture_retry_count: 0,
        capture_elapsed_ms: 0,
        capture_retry_backoff_ms: 0,
    }
}

fn set_pixel(captured: &mut CapturedWindowBgraBitmap, col: usize, row: usize, value: u8) {
    let offset = (row * FRAME_W as usize + col) * 4;
    captured.bitmap.bytes[offset] = value;
}

fn expected_hash(width: u32, height: u32, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

fn fallback_region() -> Rect {
    Rect {
        x: 100,
        y: 50,
        w: 1600,
        h: 800,
    }
}

#[test]
fn must_fail_failed_window_capture_is_unavailable_not_a_screen_fallback() {
    let evidence = window_pixel_evidence_from_capture(
        Err(CaptureError::PrintWindowDisabled {
            detail: "WGC window capture failed".to_owned(),
        }),
        fallback_region(),
        Some(Point { x: 900, y: 450 }),
        frame_origin(),
    );
    assert_eq!(evidence.window.status, "unavailable");
    assert_eq!(evidence.window.bitmap_sha256, None);
    assert!(
        evidence
            .window
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("WGC window capture failed")),
        "{:?}",
        evidence.window.detail
    );
    let point = evidence.point.expect("point evidence requested");
    assert_eq!(point.status, "unavailable");
    assert_eq!(point.bitmap_sha256, None);
}

#[test]
fn must_pass_window_capture_is_hashed_with_its_own_frame_region() {
    let captured = frame(0x22);
    let want = expected_hash(FRAME_W, FRAME_H, &captured.bitmap.bytes);
    let evidence =
        window_pixel_evidence_from_capture(Ok(captured), fallback_region(), None, frame_origin());
    assert_eq!(evidence.window.status, "ok");
    assert_eq!(evidence.window.region, frame_region());
    assert_eq!(
        evidence.window.bitmap_sha256.as_deref(),
        Some(want.as_str())
    );
    assert!(evidence.point.is_none());
}

#[test]
fn must_pass_covered_target_change_is_observed_from_its_own_pixels() {
    // The target repaints (one pixel flips) while another window covers it.
    // Window-scoped evidence sees the change; the cover plays no part.
    let before = window_pixel_evidence_from_capture(
        Ok(frame(0x10)),
        fallback_region(),
        None,
        frame_origin(),
    );
    let mut changed = frame(0x10);
    set_pixel(&mut changed, 5, 5, 0xEE);
    let after =
        window_pixel_evidence_from_capture(Ok(changed), fallback_region(), None, frame_origin());
    assert_ne!(before.window.bitmap_sha256, after.window.bitmap_sha256);
}

#[test]
fn must_fail_unchanged_target_pixels_hash_equal_so_no_false_delta() {
    let before = window_pixel_evidence_from_capture(
        Ok(frame(0x10)),
        fallback_region(),
        None,
        frame_origin(),
    );
    let after = window_pixel_evidence_from_capture(
        Ok(frame(0x10)),
        fallback_region(),
        None,
        frame_origin(),
    );
    assert_eq!(before.window, after.window);
}

#[test]
fn must_pass_point_region_is_cropped_from_the_same_window_frame() {
    // Point at screen (900, 450): the 1024x384 point rect spans x 388..1412,
    // y 258..642, fully inside the frame (x 100..1700, y 50..850).
    let point = Point { x: 900, y: 450 };
    let base = window_pixel_evidence_from_capture(
        Ok(frame(0x30)),
        fallback_region(),
        Some(point),
        frame_origin(),
    );

    // Pixel at frame col 2 / row 2 is screen (102, 52): outside the point rect.
    let mut outside = frame(0x30);
    set_pixel(&mut outside, 2, 2, 0xAA);
    let outside = window_pixel_evidence_from_capture(
        Ok(outside),
        fallback_region(),
        Some(point),
        frame_origin(),
    );
    assert_ne!(base.window.bitmap_sha256, outside.window.bitmap_sha256);
    assert_eq!(base.point, outside.point);

    // Pixel at screen (900, 450) is frame col 800 / row 400: inside the rect.
    let mut inside = frame(0x30);
    set_pixel(&mut inside, 800, 400, 0xBB);
    let inside = window_pixel_evidence_from_capture(
        Ok(inside),
        fallback_region(),
        Some(point),
        frame_origin(),
    );
    assert_ne!(
        base.point.as_ref().and_then(|p| p.bitmap_sha256.clone()),
        inside.point.as_ref().and_then(|p| p.bitmap_sha256.clone())
    );

    let point_signature = base.point.expect("point evidence requested");
    assert_eq!(point_signature.status, "ok");
    assert_eq!(
        point_signature.region,
        Rect {
            x: 388,
            y: 258,
            w: 1024,
            h: 384
        }
    );
}

#[test]
fn must_fail_point_outside_the_window_frame_is_unavailable() {
    let evidence = window_pixel_evidence_from_capture(
        Ok(frame(0x30)),
        fallback_region(),
        Some(Point {
            x: 20_000,
            y: 20_000,
        }),
        frame_origin(),
    );
    assert_eq!(evidence.window.status, "ok");
    let point = evidence.point.expect("point evidence requested");
    assert_eq!(point.status, "unavailable");
    assert_eq!(point.bitmap_sha256, None);
}
