//! Regression tests for `CapturedBgraBitmap::crop_screen_rect` (#130): the
//! browser-OCR fallback crops its tiles out of the target window's own captured
//! frame, anchored at the DWM capture bounds rather than `GetWindowRect`.

use synapse_capture::CapturedBgraBitmap;
use synapse_core::Rect;

/// BGRA frame whose pixel (x, y) encodes its own column/row in the B and G
/// channels, so crops can be asserted against exact source pixels.
fn frame_with_pixels(width: u32, height: u32, origin_x: i32, origin_y: i32) -> CapturedBgraBitmap {
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in 0..width as usize * height as usize {
        let column = (pixel % width as usize) as u8;
        let row = (pixel / width as usize) as u8;
        bytes.extend_from_slice(&[column, row, 0x80, 0xff]);
    }
    CapturedBgraBitmap {
        region: Rect {
            x: origin_x,
            y: origin_y,
            w: width as i32,
            h: height as i32,
        },
        width,
        height,
        bytes,
    }
}

/// A screen-space tile inside the frame crops to exactly the covered pixel
/// rectangle, keeping the tile's own screen rect for word anchoring.
#[test]
fn must_pass_crop_returns_covered_pixels() {
    let frame = frame_with_pixels(10, 6, 100, 50);
    let tile = Rect {
        x: 102,
        y: 51,
        w: 3,
        h: 2,
    };
    let crop = frame
        .crop_screen_rect(tile)
        .expect("interior tile must crop");
    assert_eq!((crop.width, crop.height), (3, 2));
    assert_eq!(crop.bytes.len(), 3 * 2 * 4);
    // BGRA: first cropped pixel is column 2, row 1 of the frame.
    assert_eq!(&crop.bytes[..4], &[2, 1, 0x80, 0xff]);
    // Last cropped pixel is column 4, row 2.
    assert_eq!(&crop.bytes[crop.bytes.len() - 4..], &[4, 2, 0x80, 0xff]);
}

/// Tiles are clamped to the frame and fully-outside tiles yield no pixels, so a
/// stale/oversized rect can never fabricate content.
#[test]
fn must_fail_crop_clamps_and_rejects_outside() {
    let frame = frame_with_pixels(10, 6, 100, 50);
    // Tile extends past the right/bottom edge: clamps to the frame edge.
    let tile = Rect {
        x: 108,
        y: 54,
        w: 10,
        h: 10,
    };
    let crop = frame
        .crop_screen_rect(tile)
        .expect("overhanging tile must clamp");
    assert_eq!((crop.width, crop.height), (2, 2));
    assert_eq!(crop.bytes.len(), 2 * 2 * 4);
    // The clamped crop holds the frame's bottom-right 2x2 pixels exactly.
    assert_eq!(&crop.bytes[..4], &[8, 4, 0x80, 0xff]);
    assert_eq!(&crop.bytes[crop.bytes.len() - 4..], &[9, 5, 0x80, 0xff]);
    // Tile entirely beyond the frame: no pixels.
    let outside = Rect {
        x: 200,
        y: 200,
        w: 5,
        h: 5,
    };
    assert!(frame.crop_screen_rect(outside).is_none());
    // Empty tile: no pixels.
    let empty = Rect {
        x: 100,
        y: 50,
        w: 0,
        h: 4,
    };
    assert!(frame.crop_screen_rect(empty).is_none());
}

/// #130 review follow-up: a WGC frame anchors at the DWM extended frame
/// bounds, which sit inside the `GetWindowRect` bounds by the invisible resize
/// border. Browser content tiles derived from the frame's own (DWM-anchored)
/// region must crop exactly, and window-rect-derived tiles must clamp into the
/// frame's true pixels instead of reading past its edge.
#[test]
fn must_pass_crop_aligns_tiles_to_dwm_anchored_frame() {
    let window_bounds = Rect {
        x: 13,
        y: 13,
        w: 1376,
        h: 1001,
    };
    let border = 7;
    // DWM bounds: inset left/right (and bottom) by the invisible border; the
    // top matches for a normal window.
    let frame = frame_with_pixels(
        (window_bounds.w - 2 * border) as u32,
        (window_bounds.h - border) as u32,
        window_bounds.x + border,
        window_bounds.y,
    );
    assert_eq!(frame.region.w, frame.width as i32);

    let chrome_top = 96;
    let content = Rect {
        x: frame.region.x,
        y: frame.region.y + chrome_top,
        w: frame.region.w,
        h: frame.region.h - chrome_top,
    };
    let tile_height = 600;
    let mut tiles = 0;
    let mut y = content.y;
    while y < content.y + content.h && tiles < 8 {
        let tile = Rect {
            x: content.x,
            y,
            w: content.w,
            h: tile_height.min(content.y + content.h - y),
        };
        let crop = frame
            .crop_screen_rect(tile)
            .expect("DWM-aligned tile must crop without clamping");
        // Inside the DWM-anchored frame the crop is exact, so OCR word boxes
        // anchored to the tile rect land on the target's true pixels.
        assert_eq!(crop.width as i32, tile.w);
        assert_eq!(crop.height as i32, tile.h);
        y += tile_height;
        tiles += 1;
    }
    assert!(tiles >= 2, "expected multiple OCR tiles, got {tiles}");

    // A tile still expressed in window-rect coordinates (the pre-fix shape)
    // clamps into the frame instead of failing or fabricating border pixels.
    let stale_tile = Rect {
        x: window_bounds.x,
        y: window_bounds.y + chrome_top,
        w: window_bounds.w,
        h: tile_height,
    };
    let clamped = frame
        .crop_screen_rect(stale_tile)
        .expect("window-rect tile must clamp into the DWM frame");
    // Clamped to the frame's true pixels: the 7 px border column on the left
    // and the overhang on the right are cut, the full 600 px height fits.
    assert_eq!(clamped.width as i32, frame.region.w);
    assert_eq!(clamped.height, 600);
    assert_eq!(clamped.bytes.len(), frame.region.w as usize * 600 * 4);
    // First pixel is frame column 0 (not a fabricated border pixel), row 96.
    assert_eq!(&clamped.bytes[..4], &[0, 96, 0x80, 0xff]);
}
