//! #49: WGC frames must be copied into OWNED CPU buffers on the producer
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! (capture) thread; consumers only ever read plain bytes, so a frame stays
//! valid after the capture session, its D3D11 device and textures are gone.
//! No blanket `unsafe impl Send/Sync` (`SendablePtr`) may come back.
//!
//! #47: the WGC frame callback must not run a whole-desktop UIA classification
//! per frame: the capture crate has no UIA/a11y dependency and continuous
//! capture keeps its cadence while many other windows are visible.
//!
//! Live parts capture only test-owned solid-colour windows (no owner screen
//! content). Contract: `src/platform/windows/capture.rs` `on_frame_arrived`
//! -> `readback.rs` -> `controller.rs` `push_frame`; `src/frame.rs`
//! `CapturedFrame` / `CapturedFrameBuffer`.

#![cfg(windows)]

#[path = "../../synapse-mcp/tests/g6a_window/mod.rs"]
mod g6a_window;

use std::{
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use g6a_window::{BLUE, GREEN, PaintedWindow, RED, WindowSpec};
use synapse_capture::{
    CaptureConfig, CaptureTarget, CapturedFrame, DxgiFormat, captured_frame_region_to_bgra_bitmap,
    spawn_capture_loop,
};
use synapse_core::Rect;

static LIVE_CAPTURE: Mutex<()> = Mutex::new(());

fn window_config(hwnd: i64) -> CaptureConfig {
    CaptureConfig {
        target: CaptureTarget::Window { hwnd },
        min_update_interval_ms: 16,
        cursor_visible: false,
        dirty_region_only: false,
        ..CaptureConfig::default()
    }
}

/// Centre pixel as `[r, g, b]`, honouring the frame's channel order.
fn centre_rgb(frame: &CapturedFrame) -> [u8; 3] {
    let stride = frame.pixels.row_stride_bytes;
    let offset = (frame.height as usize / 2) * stride + (frame.width as usize / 2) * 4;
    let px = &frame.pixels.bytes[offset..offset + 4];
    match frame.format {
        DxgiFormat::Rgba8 | DxgiFormat::Rgba8Srgb => [px[0], px[1], px[2]],
        _ => [px[2], px[1], px[0]],
    }
}

/// Receives frames until one shows `color` (or the deadline passes).
fn frame_showing(
    rx: &crossbeam::channel::Receiver<CapturedFrame>,
    color: u32,
) -> Option<CapturedFrame> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(frame) = rx.recv_timeout(Duration::from_millis(100))
            && g6a_window::classify_rgb(centre_rgb(&frame)) == Some(color)
        {
            return Some(frame);
        }
    }
    None
}

#[test]
fn must_pass_window_frames_keep_exact_owned_pixels_after_capture_stops() {
    let _live = LIVE_CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = PaintedWindow::open(WindowSpec::visible(240, 180, RED));
    let handle =
        spawn_capture_loop(window_config(window.hwnd())).expect("start WGC window capture");
    let rx = handle.receiver();
    let red = frame_showing(&rx, RED).expect("a RED frame of the test window");
    window.set_color(GREEN);
    assert!(
        window.wait_painted(GREEN, Duration::from_secs(5)),
        "window repainted GREEN"
    );
    let green = frame_showing(&rx, GREEN).expect("a fresh GREEN frame after repaint");
    handle.stop().expect("stop capture");
    drop(rx);

    for (frame, color) in [(&red, RED), (&green, GREEN)] {
        let row = frame.width as usize * usize::from(frame.pixels.bytes_per_pixel);
        assert_eq!(
            frame.pixels.row_stride_bytes, row,
            "tightly packed owned rows"
        );
        assert_eq!(frame.pixels.bytes.len(), row * frame.height as usize);
        assert_eq!(g6a_window::classify_rgb(centre_rgb(frame)), Some(color));
        let centre = Rect {
            x: i32::try_from(frame.width / 2).unwrap_or(0) - 5,
            y: i32::try_from(frame.height / 2).unwrap_or(0) - 5,
            w: 10,
            h: 10,
        };
        let crop = captured_frame_region_to_bgra_bitmap(frame, centre).expect("consumer crop");
        assert_eq!(crop.bytes.len(), 10 * 10 * 4);
        let expected = g6a_window::classify_rgb(centre_rgb(frame));
        assert!(
            crop.bytes
                .chunks_exact(4)
                .all(|bgra| g6a_window::classify_rgb([bgra[2], bgra[1], bgra[0]]) == expected),
            "#49: a consumer crop after capture shutdown must read the owned pixels"
        );
    }
    assert!(
        red.frame_seq < green.frame_seq,
        "the GREEN frame is a later, fresh frame"
    );
}

#[test]
fn must_pass_captured_frame_is_plain_owned_data() {
    fn owned<T: Send + Sync + 'static>() {}
    owned::<CapturedFrame>();
    owned::<synapse_capture::CapturedFrameBuffer>();
}

fn capture_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read capture src").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("read source");
                out.push((path.display().to_string(), text));
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out
}

#[test]
fn must_fail_guard_no_blanket_unsafe_send_or_raw_texture_frames() {
    let sources = capture_sources();
    assert!(sources.len() > 5, "precondition: capture sources found");
    for (path, text) in &sources {
        for banned in [
            "unsafe impl Send",
            "unsafe impl<T> Send",
            "unsafe impl Sync",
            "SendablePtr",
        ] {
            assert!(!text.contains(banned), "#49: {path} contains `{banned}`");
        }
    }
    let frame_rs = sources
        .iter()
        .find(|(p, _)| p.ends_with("frame.rs"))
        .expect("frame.rs");
    assert!(
        !frame_rs.1.contains("ID3D11Texture2D"),
        "#49: CapturedFrame must not hold a GPU texture"
    );
}

#[test]
fn must_fail_guard_wgc_callback_crate_has_no_uia_path() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("read Cargo.toml");
    for banned in ["synapse-a11y", "uiautomation"] {
        assert!(
            !manifest.contains(banned),
            "#47: synapse-capture depends on {banned}"
        );
    }
    for (path, text) in capture_sources() {
        for banned in [
            "IUIAutomation",
            "UI_Accessibility",
            "synapse_a11y",
            "EnumWindows",
            "FindAll",
        ] {
            assert!(
                !text.contains(banned),
                "#47: {path} contains `{banned}` (UIA/desktop scan)"
            );
        }
    }
}

#[test]
fn must_pass_window_capture_keeps_cadence_with_many_visible_windows() {
    let _live = LIVE_CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let crowd: Vec<PaintedWindow> = (0..12)
        .map(|i| {
            PaintedWindow::open(WindowSpec {
                x: 80 + i * 12,
                y: 80 + i * 12,
                ..WindowSpec::visible(160, 120, BLUE)
            })
        })
        .collect();
    assert_eq!(crowd.len(), 12, "precondition: 12 extra visible windows");
    let window = PaintedWindow::open(WindowSpec::visible(240, 180, RED));
    let handle =
        spawn_capture_loop(window_config(window.hwnd())).expect("start WGC window capture");
    let rx = handle.receiver();
    assert!(
        frame_showing(&rx, RED).is_some(),
        "precondition: first frame arrived"
    );

    let started = Instant::now();
    let mut frames = 0_u32;
    let mut flip = false;
    let limit = Duration::from_millis(1_200);
    while started.elapsed() < limit {
        flip = !flip;
        window.set_color(if flip { GREEN } else { RED });
        // Repaint again after each receive, even while WGC keeps producing
        // frames. Draining until a timeout can starve the repaint producer.
        if rx.recv_timeout(Duration::from_millis(40)).is_ok() {
            frames += 1;
        }
    }
    let dropped = handle.stats().frames_dropped();
    handle.stop().expect("stop capture");
    assert!(
        frames >= 8,
        "#47: a repainting window must stream frames (got {frames} in 1.2 s, dropped {dropped})"
    );
}
