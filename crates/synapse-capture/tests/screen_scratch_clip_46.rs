//! Regression for #46: the per-thread screen scratch DIB is recycled between
//! captures; a BitBlt from an off-desktop source rect is clipped by GDI and
//! must not leave the previous capture's pixels in the returned bytes.

#![cfg(windows)]

use synapse_capture::screen_region_to_bgra_bitmap;
use synapse_core::Rect;

#[test]
fn must_fail_fully_offscreen_region_returns_no_stale_pixels() {
    // Same-size visible capture first so the recycled scratch DIB (thread-local,
    // matched by width/height) holds real desktop pixels. Precondition: those
    // pixels are not all zero, otherwise stale reuse would be invisible. Try a
    // few on-screen spots (a black corner is common); the last successful grab
    // is what stays in the scratch.
    let mut primed = false;
    for (x, y) in [(0, 0), (100, 100), (400, 300), (800, 500), (200, 600)] {
        let visible = match screen_region_to_bgra_bitmap(Rect { x, y, w: 64, h: 64 }) {
            Ok(bitmap) => bitmap,
            Err(error) => panic!("visible capture at ({x},{y}) failed: {error:?}"),
        };
        if visible.bytes.iter().any(|byte| *byte != 0) {
            primed = true;
            break;
        }
    }
    assert!(
        primed,
        "precondition: no visible 64x64 grab had non-zero pixels (locked, secure or all-black desktop), so stale reuse cannot be detected"
    );

    let offscreen = Rect {
        x: -1_000_000,
        y: -1_000_000,
        w: 64,
        h: 64,
    };
    // #46 contract (criteria 2026-10-17): a region wholly outside the virtual
    // screen is refused with a typed error, so no stale (or black) pixels are
    // ever returned as a capture.
    match screen_region_to_bgra_bitmap(offscreen) {
        Ok(bitmap) => panic!(
            "off-screen region must be refused, got {} bytes",
            bitmap.bytes.len()
        ),
        Err(error) => assert_eq!(
            error.code(),
            synapse_core::error_codes::CAPTURE_TARGET_INVALID,
            "{error:?}"
        ),
    }
}

#[test]
fn must_pass_on_screen_region_returns_exact_bgra_dimensions() {
    let region = Rect {
        x: 0,
        y: 0,
        w: 32,
        h: 16,
    };
    let bitmap = match screen_region_to_bgra_bitmap(region) {
        Ok(bitmap) => bitmap,
        Err(error) => panic!("on-screen capture failed: {error:?}"),
    };

    assert_eq!(bitmap.region, region);
    assert_eq!(bitmap.width, 32);
    assert_eq!(bitmap.height, 16);
    assert_eq!(bitmap.bytes.len(), 32 * 16 * 4);

    // Recycled scratch of a different size must not change the next result's
    // shape: a second, larger on-screen grab is sized to its own region.
    let larger = Rect {
        x: 0,
        y: 0,
        w: 48,
        h: 24,
    };
    let second = match screen_region_to_bgra_bitmap(larger) {
        Ok(bitmap) => bitmap,
        Err(error) => panic!("second on-screen capture failed: {error:?}"),
    };
    assert_eq!(second.region, larger);
    assert_eq!(second.bytes.len(), 48 * 24 * 4);
}
