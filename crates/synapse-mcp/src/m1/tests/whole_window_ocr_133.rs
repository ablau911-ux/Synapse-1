//! Regression tests for whole-window OCR coordinate mapping (#133).
//!
//! `ReadTextCaptureSource::WholeWindow` rewrites the OCR region to the captured
//! bitmap's native extent (bitmap-relative origin 0,0). The bitmap extent comes
//! from the DWM frame bounds, so it differs from the `GetWindowRect` extent for
//! hidden-desktop (PrintWindow) windows and scales with DPI. The OCR region must
//! follow the bitmap, never the window rect or the screen anchor.

use synapse_core::{OcrBackend, Rect};

use super::{
    ReadTextCaptureSource, ResolvedReadTextRequest, read_text_request_for_captured_bitmap,
};

fn request(capture_source: ReadTextCaptureSource, region: Rect) -> ResolvedReadTextRequest {
    ResolvedReadTextRequest {
        region,
        capture_source,
        requested_backend: OcrBackend::Auto,
        effective_backend: OcrBackend::Winrt,
        lang_hint: None,
        synthetic: false,
    }
}

fn whole_window_request() -> ResolvedReadTextRequest {
    request(
        ReadTextCaptureSource::WholeWindow { hwnd: 0x1234 },
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
    )
}

fn bitmap(screen_anchor: Rect, width: u32, height: u32) -> synapse_capture::CapturedBgraBitmap {
    synapse_capture::CapturedBgraBitmap {
        region: screen_anchor,
        width,
        height,
        bytes: Vec::new(),
    }
}

#[test]
fn must_pass_whole_window_region_equals_bitmap_extent_at_every_dpi_scale() {
    // 1502x1039 is the DWM frame / PrintWindow bitmap from the #133 report.
    for (label, scale_percent) in [("125%", 125_u32), ("150%", 150), ("200%", 200)] {
        let width = 1502 * scale_percent / 100;
        let height = 1039 * scale_percent / 100;
        let captured = bitmap(
            Rect {
                x: 0,
                y: 0,
                w: i32::try_from(width).expect("width fits"),
                h: i32::try_from(height).expect("height fits"),
            },
            width,
            height,
        );

        let resolved = read_text_request_for_captured_bitmap(whole_window_request(), &captured)
            .unwrap_or_else(|error| panic!("{label}: valid extent must resolve: {error:?}"));

        assert_eq!(
            resolved.region,
            Rect {
                x: 0,
                y: 0,
                w: i32::try_from(width).expect("width fits"),
                h: i32::try_from(height).expect("height fits"),
            },
            "{label}"
        );
    }
}

#[test]
fn must_fail_hidden_desktop_bitmap_smaller_than_getwindowrect_is_not_inflated() {
    // GetWindowRect-style bounds 1518x1047; PrintWindow bitmap 1502x1039. The
    // request must carry the bitmap extent, so OCR never reads past the bitmap.
    // The bitmap is also anchored away from the screen origin (DWM bounds).
    let captured = bitmap(
        Rect {
            x: 8,
            y: 31,
            w: 1502,
            h: 1039,
        },
        1502,
        1039,
    );

    let resolved = read_text_request_for_captured_bitmap(whole_window_request(), &captured)
        .expect("bitmap extent is valid");

    assert_eq!(
        resolved.region,
        Rect {
            x: 0,
            y: 0,
            w: 1502,
            h: 1039,
        }
    );
}

#[test]
fn must_pass_whole_window_request_preserves_other_fields_and_source() {
    let captured = bitmap(
        Rect {
            x: 0,
            y: 0,
            w: 1502,
            h: 1039,
        },
        1502,
        1039,
    );

    let resolved = read_text_request_for_captured_bitmap(whole_window_request(), &captured)
        .expect("bitmap extent is valid");

    assert_eq!(
        resolved.capture_source,
        ReadTextCaptureSource::WholeWindow { hwnd: 0x1234 }
    );
    assert_eq!(resolved.effective_backend, OcrBackend::Winrt);
}

#[test]
fn must_pass_explicit_window_region_passes_through_unchanged() {
    let region = Rect {
        x: 10,
        y: 20,
        w: 300,
        h: 100,
    };
    let captured = bitmap(region, 300, 100);

    let resolved = read_text_request_for_captured_bitmap(
        request(
            ReadTextCaptureSource::Window {
                hwnd: 0x1234,
                window_region: region,
            },
            region,
        ),
        &captured,
    )
    .expect("explicit region passes through");

    assert_eq!(resolved.region, region);
}

#[test]
fn must_fail_empty_or_oversized_whole_window_bitmap_is_rejected() {
    let empty = bitmap(
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        0,
        0,
    );
    let empty_error = read_text_request_for_captured_bitmap(whole_window_request(), &empty)
        .expect_err("a 0x0 whole-window bitmap must be refused");
    assert_eq!(
        error_code(&empty_error),
        Some(synapse_core::error_codes::OCR_NO_TEXT)
    );
    assert!(
        empty_error.message.contains("must be non-empty"),
        "{}",
        empty_error.message
    );

    let oversized = bitmap(
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        u32::MAX,
        10,
    );
    let oversized_error = read_text_request_for_captured_bitmap(whole_window_request(), &oversized)
        .expect_err("a width past i32 must be refused");
    assert_eq!(
        error_code(&oversized_error),
        Some(synapse_core::error_codes::OCR_NO_TEXT)
    );
    assert!(
        oversized_error
            .message
            .contains("width 4294967295 exceeds i32"),
        "{}",
        oversized_error.message
    );
}

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}
