//! #179 (re-review PARTIAL): `decode_png_to_bgra` allocates
//! `reader.output_buffer_size()` for whatever size the PNG header claims; png
//! 0.18 `Limits` do not cover that caller-owned buffer. The decode must refuse
//! an over-budget header BEFORE allocating.
//!
//! NEEDS-SEAM (fixer adds it in `cdp_action.rs`):
//! `fn decode_png_to_bgra_bounded(png_bytes: &[u8], max_pixels: u64) -> A11yResult<CdpNodeBitmap>`;
//! `decode_png_to_bgra` delegates to it with the 100_000_000 px default
//! (`synapse-mcp/src/server/browser_screenshot_limits.rs` DEFAULT_MAX_PIXELS).
//! Over-budget fixtures claim 5000x5000 (100 MB) against a 1_000_000 px budget,
//! so even a wrong implementation never allocates gigabytes.

use synapse_core::error_codes;

use super::{decode_png_to_bgra, decode_png_to_bgra_bounded};

// ---- PNG fixtures -------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&u32::try_from(data.len()).expect("chunk len").to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// A structurally valid RGBA8 PNG whose header CLAIMS `w`x`h` but carries an
/// empty zlib stream: only the header decides the output-buffer size.
fn header_claiming(w: u32, h: u32) -> Vec<u8> {
    let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(
        &mut png,
        b"IDAT",
        &[0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    );
    chunk(&mut png, b"IEND", &[]);
    png
}

fn small_rgba_png() -> Vec<u8> {
    let pixels: Vec<u8> = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [9, 8, 7, 255],
    ]
    .concat();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 2, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&pixels).expect("png data");
    drop(writer);
    out
}

#[test]
fn must_pass_small_png_decodes_to_exact_bgra() {
    let bitmap = decode_png_to_bgra(&small_rgba_png()).expect("decode 2x2 PNG");
    assert_eq!((bitmap.width, bitmap.height), (2, 2));
    let expected: Vec<u8> = [
        [0, 0, 255, 255],
        [0, 255, 0, 255],
        [255, 0, 0, 255],
        [7, 8, 9, 255],
    ]
    .concat();
    assert_eq!(bitmap.bgra, expected);
}

#[test]
fn must_pass_bounded_decode_accepts_a_png_inside_the_budget() {
    let bitmap = decode_png_to_bgra_bounded(&small_rgba_png(), 4).expect("4 px fits a 4 px budget");
    assert_eq!(bitmap.bgra.len(), 2 * 2 * 4);
}

#[test]
fn must_fail_png_header_over_the_pixel_budget_is_refused_before_allocation() {
    // 5000x5000 claims 100 MB of RGBA; the budget is 1_000_000 px.
    let error = match decode_png_to_bgra_bounded(&header_claiming(5_000, 5_000), 1_000_000) {
        Ok(bitmap) => panic!(
            "#179: over-budget PNG decoded to {}x{}",
            bitmap.width, bitmap.height
        ),
        Err(error) => error,
    };
    assert_eq!(error.code(), error_codes::A11Y_CDP_AXTREE_FAILED, "{error}");
    let detail = error.to_string();
    assert!(
        detail.contains("exceeds") && detail.contains("5000x5000"),
        "#179: refusal must name the claimed size and the limit: {detail}"
    );
}

#[test]
fn must_fail_bounded_decode_budget_boundary_is_inclusive() {
    assert!(decode_png_to_bgra_bounded(&small_rgba_png(), 4).is_ok_and(|b| b.width == 2));
    let error = match decode_png_to_bgra_bounded(&small_rgba_png(), 3) {
        Ok(_) => panic!("#179: a 4 px PNG must not fit a 3 px budget"),
        Err(error) => error,
    };
    assert_eq!(error.code(), error_codes::A11Y_CDP_AXTREE_FAILED, "{error}");
}
