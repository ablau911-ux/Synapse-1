//! #224: a minimized window never presents a frame, so capture must refuse it
//! promptly as `CAPTURE_TARGET_MINIMIZED` (no WGC wait; criteria 2026-10-17)
//! through the real capture entry
//! points, never as `CAPTURE_PRINTWINDOW_DISABLED` (which names the wrong
//! cause). Previous tests only called the private mapper with a hand-built
//! error; these drive `window_region_to_bgra_bitmap` and
//! `window_full_frame_to_bgra_bitmap` on a real, test-owned minimized window.
//!
//! Contract: `src/platform/windows/bitmap.rs` `wgc_failure_to_capture_error`
//! (timeout keeps its own code) and `src/error.rs` code mapping.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../synapse-mcp/tests/g6a_window/mod.rs"]
mod g6a_window;

use g6a_window::{PaintedWindow, RED, WindowSpec};
use synapse_capture::{window_full_frame_to_bgra_bitmap, window_region_to_bgra_bitmap};
use synapse_core::{Rect, error_codes};

const TIMEOUT_MS: u64 = 300;
const REGION: Rect = Rect {
    x: 10,
    y: 10,
    w: 40,
    h: 30,
};

#[test]
fn must_fail_minimized_region_capture_reports_target_minimized() {
    let window = PaintedWindow::open(WindowSpec::minimized(RED));
    assert!(window.is_minimized(), "precondition: target is minimized");
    let error = match window_region_to_bgra_bitmap(window.hwnd(), REGION, TIMEOUT_MS) {
        Ok(bitmap) => panic!(
            "a minimized window must not yield a frame: {}",
            bitmap.capture_backend
        ),
        Err(error) => error,
    };
    assert_eq!(
        error.code(),
        error_codes::CAPTURE_TARGET_MINIMIZED,
        "#224: minimized-window timeout misreported: {error}"
    );
    assert!(
        error
            .to_string()
            .contains("is minimized; restore it before capturing"),
        "the detail must name the minimized target: {error}"
    );
}

#[test]
fn must_fail_minimized_full_frame_capture_reports_target_minimized() {
    let window = PaintedWindow::open(WindowSpec::minimized(RED));
    assert!(window.is_minimized(), "precondition: target is minimized");
    let error = match window_full_frame_to_bgra_bitmap(window.hwnd(), TIMEOUT_MS) {
        Ok(bitmap) => panic!(
            "a minimized window must not yield a frame: {:?}",
            bitmap.capture_backend
        ),
        Err(error) => error,
    };
    assert_eq!(
        error.code(),
        error_codes::CAPTURE_TARGET_MINIMIZED,
        "{error}"
    );
    assert_ne!(
        error.code(),
        error_codes::CAPTURE_PRINTWINDOW_DISABLED,
        "{error}"
    );
}

#[test]
fn must_pass_visible_window_region_capture_returns_exact_pixels() {
    let window = PaintedWindow::open(WindowSpec::visible(200, 150, RED));
    assert!(
        window.wait_captured_color(RED, std::time::Duration::from_secs(5)),
        "precondition: compositor presents the RED fixture before region capture"
    );
    let captured = match window_region_to_bgra_bitmap(window.hwnd(), REGION, 1_500) {
        Ok(captured) => captured,
        Err(error) => panic!("visible test window must capture: {error}"),
    };
    assert_eq!(captured.capture_backend, "graphics_capture_window_bgra");
    assert_eq!((captured.bitmap.width, captured.bitmap.height), (40, 30));
    assert_eq!(captured.bitmap.bytes.len(), 40 * 30 * 4);
    assert!(
        captured
            .bitmap
            .bytes
            .chunks_exact(4)
            .all(|bgra| bgra[2] > 200 && bgra[1] < 60 && bgra[0] < 60),
        "every captured pixel must be the window's RED (BGRA)"
    );
}
