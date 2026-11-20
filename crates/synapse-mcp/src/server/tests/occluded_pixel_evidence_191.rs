//! #191 through the REAL verify_delta capture callers on a real occluded window.
//!
//! The test creates its own target window T and its own cover window C at the
//! same screen rectangle (C above T), so T is fully occluded. A screen-DC
//! capture at T's bounds would show C. The pixel evidence the callers record
//! must come from T itself (WGC window capture):
//! - C repainting must NOT change T's evidence (no false delta),
//! - T changing under C MUST change T's evidence (no false "no delta").
//!
//! Drives `capture_coordinate_click_delta_signature` and
//! `capture_action_delta_signature` (the callers named in the issue). Needs an
//! interactive, unlocked desktop with WGC; a capture that is `unavailable`
//! fails the test with that reason instead of passing vacuously.

use std::time::{Duration, Instant};

use super::*;
use crate::server::own_window_fixture::{OwnWindows, WindowSpec};

const TARGET: usize = 0;
const COVER: usize = 1;
const POLL: Duration = Duration::from_secs(4);

fn covered_target() -> OwnWindows {
    let spec = |label| WindowSpec {
        x: 96,
        y: 96,
        w: 320,
        h: 200,
        label: Some(label),
    };
    let windows = OwnWindows::spawn(vec![spec("target before"), spec("cover A")]);
    assert_eq!(
        crate::m2::root_hwnd_at_point(windows.center(TARGET)),
        Some(windows.hwnd(COVER)),
        "precondition: the target must be fully covered by the test's cover window"
    );
    windows
}

fn service() -> (tempfile::TempDir, SynapseService) {
    let dir = tempfile::tempdir().expect("tmp");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("service");
    (dir, service)
}

fn pixels(service: &SynapseService, hwnd: i64) -> ClickPixelSignature {
    let signature = service
        .capture_coordinate_click_delta_signature(hwnd)
        .expect("coordinate click signature");
    assert_eq!(
        signature.foreground_hwnd, hwnd,
        "observed window is the requested one"
    );
    assert_eq!(
        signature.pixel.status, "ok",
        "window capture unavailable (needs an interactive, unlocked desktop with WGC): {:?}",
        signature.pixel.detail
    );
    signature.pixel
}

/// Polls `read` until `changed` holds or the deadline passes; returns the last read.
fn poll_until<T>(mut read: impl FnMut() -> T, changed: impl Fn(&T) -> bool) -> T {
    let deadline = Instant::now() + POLL;
    loop {
        let value = read();
        if changed(&value) || Instant::now() >= deadline {
            return value;
        }
    }
}

/// Repaints the cover and waits until the cover's own pixels really changed,
/// so the screen at the target's bounds is proven different.
fn repaint_cover(service: &SynapseService, windows: &OwnWindows, text: &str) {
    let before = pixels(service, windows.hwnd(COVER));
    windows.set_label_text(COVER, text);
    let after = poll_until(|| pixels(service, windows.hwnd(COVER)), |p| *p != before);
    assert_ne!(
        after, before,
        "precondition: the cover window must have repainted"
    );
}

#[test]
fn must_fail_cover_repaint_is_not_a_target_pixel_change() {
    let windows = covered_target();
    let (_dir, service) = service();
    let before = pixels(&service, windows.hwnd(TARGET));

    repaint_cover(&service, &windows, "cover B, repainted by the test");

    assert_eq!(pixels(&service, windows.hwnd(TARGET)), before);
}

#[test]
fn must_pass_change_of_the_covered_target_is_observed() {
    let windows = covered_target();
    let (_dir, service) = service();
    let before = pixels(&service, windows.hwnd(TARGET));

    windows.set_label_text(TARGET, "target AFTER, changed under the cover");
    let after = poll_until(|| pixels(&service, windows.hwnd(TARGET)), |p| *p != before);

    assert_ne!(after.bitmap_sha256, before.bitmap_sha256);
    assert_eq!(
        crate::m2::root_hwnd_at_point(windows.center(TARGET)),
        Some(windows.hwnd(COVER)),
        "the target stayed covered while it changed"
    );
}

async fn action_signature(service: &SynapseService, windows: &OwnWindows) -> ClickDeltaSignature {
    let point = windows.center(TARGET);
    let signature = service
        .capture_action_delta_signature(160, Some(point), false, Some(windows.hwnd(TARGET)))
        .await
        .expect("action delta signature");
    assert_eq!(signature.foreground_hwnd, windows.hwnd(TARGET));
    assert_eq!(signature.pixel.status, "ok", "{:?}", signature.pixel.detail);
    let point_pixel = signature
        .point_pixel
        .clone()
        .expect("point region requested");
    assert_eq!(point_pixel.status, "ok", "{:?}", point_pixel.detail);
    signature
}

#[tokio::test]
async fn must_fail_action_signature_pixels_ignore_the_cover() {
    let windows = covered_target();
    let (_dir, service) = service();
    let before = action_signature(&service, &windows).await;

    repaint_cover(&service, &windows, "cover C, repainted by the test");
    let after = action_signature(&service, &windows).await;

    assert_eq!(after.pixel, before.pixel);
    assert_eq!(after.point_pixel, before.point_pixel);
}

#[tokio::test]
async fn must_pass_action_signature_pixels_see_the_covered_target_change() {
    let windows = covered_target();
    let (_dir, service) = service();
    let before = action_signature(&service, &windows).await;

    windows.set_label_text(TARGET, "target AFTER, changed under the cover");
    let deadline = Instant::now() + POLL;
    let mut after = action_signature(&service, &windows).await;
    while after.point_pixel == before.point_pixel && Instant::now() < deadline {
        after = action_signature(&service, &windows).await;
    }

    assert_ne!(after.pixel, before.pixel);
    assert_ne!(after.point_pixel, before.point_pixel);
}
