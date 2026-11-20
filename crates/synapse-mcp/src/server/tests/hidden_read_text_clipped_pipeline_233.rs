//! Regression tests for #233 driving the clipped client-region OCR pipeline
//! (`client_region_to_window_regions` -> `hidden_desktop_read_text_capture_plan`
//! -> `hidden_desktop_read_text_request_region` ->
//! `SynapseService::read_text_request_with_captured_bitmap`) and
//! `resolve_read_text_request` on a real Win32 window.

use anyhow::{Context, ensure};
use synapse_core::{OcrBackend, Rect, error_codes};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SW_SHOWNOACTIVATE, ShowWindow, WS_EX_NOACTIVATE, WS_POPUP,
};
use windows::core::w;

use super::{
    CapturedOcrBitmap, ReadTextParams, hidden_desktop_read_text_capture_plan,
    hidden_desktop_read_text_request_region, resolve_read_text_request,
};
use crate::m1::M1State;
use crate::test_support::{PerMonitorDpi, PumpedWindows, from_hwnd};

/// Popup owned by a message-pumping UI thread, so a concurrent UIA query that
/// reaches it (e.g. a point lookup at its screen position) is answered. It is
/// created per-monitor-v2 DPI aware and every test reads its geometry under
/// [`PerMonitorDpi`]: in this DPI-unaware test process `GetClientRect` and
/// `ClientToScreen` would otherwise be logical while the DWM frame bounds are
/// physical, skewing the client offset at 125% scale.
struct TestPopupWindow {
    ui: PumpedWindows,
}

impl TestPopupWindow {
    fn open(w: i32, h: i32) -> anyhow::Result<Self> {
        let ui = PumpedWindows::spawn(move || {
            let _dpi = PerMonitorDpi::enter();
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("Static"),
                    w!("synapse-233-clip-window"),
                    WS_POPUP,
                    80,
                    80,
                    w,
                    h,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .context("CreateWindowExW")?;
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            Ok(vec![from_hwnd(hwnd)])
        })?;
        Ok(Self { ui })
    }

    fn hwnd_i64(&self) -> i64 {
        self.ui.hwnd(0)
    }
}

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn must_pass_clipped_client_region_anchors_hidden_desktop_ocr_result_and_words()
-> anyhow::Result<()> {
    let dir = tempfile::TempDir::new().context("tempdir")?;
    let service = super::tests::service_with_temp_db(dir.path())?;
    let _dpi = PerMonitorDpi::enter();
    let win = TestPopupWindow::open(300, 160)?;
    let hwnd = win.hwnd_i64();

    let requested = Rect {
        x: -20,
        y: -15,
        w: 320,
        h: 150,
    };
    let clipped = synapse_capture::client_region_to_window_regions(hwnd, requested)
        .context("client_region_to_window_regions")?;
    ensure!(
        clipped.client
            == Rect {
                x: 0,
                y: 0,
                w: 300,
                h: 135
            }
            && clipped.client != requested,
        "#233: expected (-20,-15,320,150) on 300x160 popup to clip to (0,0,300,135), got {:?}",
        clipped.client
    );

    let plan = hidden_desktop_read_text_capture_plan(Some(requested));
    let request = crate::m1::ResolvedReadTextRequest {
        region: hidden_desktop_read_text_request_region(&plan, Some(clipped.client)),
        capture_source: crate::m1::ReadTextCaptureSource::Window {
            hwnd,
            window_region: clipped.window,
        },
        requested_backend: OcrBackend::Auto,
        effective_backend: OcrBackend::Winrt,
        lang_hint: None,
        synthetic: true,
    };
    let captured = CapturedOcrBitmap {
        bitmap: synapse_capture::CapturedBgraBitmap {
            region: clipped.window,
            width: u32::try_from(clipped.window.w).context("width")?,
            height: u32::try_from(clipped.window.h).context("height")?,
            bytes: Vec::new(),
        },
        capture_source: "window",
        capture_backend: "printwindow",
        capture_hwnd: Some(hwnd),
        capture_region: clipped.window,
    };

    let result = service
        .read_text_request_with_captured_bitmap(request, captured)
        .map_err(|error| {
            anyhow::anyhow!("read_text_request_with_captured_bitmap: {}", error.message)
        })?;
    let first_word = result.words.first().context("synthetic OCR word")?;
    ensure!(
        result.region == clipped.client
            && first_word.bbox
                == Rect {
                    x: clipped.client.x + 4,
                    y: clipped.client.y + 4,
                    w: 72,
                    h: 18,
                },
        "#233: OCR result and word bbox must be anchored to clipped client region {:?}, got region={:?} word={:?}",
        clipped.client,
        result.region,
        first_word.bbox
    );
    Ok(())
}

#[test]
fn must_pass_normal_window_resolve_read_text_request_returns_clipped_client_region()
-> anyhow::Result<()> {
    let _dpi = PerMonitorDpi::enter();
    let win = TestPopupWindow::open(300, 160)?;
    let hwnd = win.hwnd_i64();
    let params = ReadTextParams {
        region: Some(Rect {
            x: 180,
            y: 90,
            w: 250,
            h: 140,
        }),
        element_id: None,
        window_hwnd: Some(hwnd),
        backend: OcrBackend::Auto,
        lang_hint: None,
    };

    let resolved = resolve_read_text_request(&M1State::default(), &params, None)
        .map_err(|error| anyhow::anyhow!("resolve_read_text_request: {}", error.message))?;
    let expected = Rect {
        x: 180,
        y: 90,
        w: 120,
        h: 70,
    };
    ensure!(
        resolved.region == expected
            && resolved.capture_source
                == crate::m1::ReadTextCaptureSource::Window {
                    hwnd,
                    window_region: expected,
                },
        "#233: normal window read_text must clip overflowing client region to {expected:?}, got {resolved:?}"
    );
    Ok(())
}

#[test]
fn must_fail_client_region_completely_outside_window_bitmap_is_rejected() -> anyhow::Result<()> {
    let _dpi = PerMonitorDpi::enter();
    let win = TestPopupWindow::open(300, 160)?;
    let hwnd = win.hwnd_i64();
    let params = ReadTextParams {
        region: Some(Rect {
            x: 400,
            y: 200,
            w: 100,
            h: 50,
        }),
        element_id: None,
        window_hwnd: Some(hwnd),
        backend: OcrBackend::Auto,
        lang_hint: None,
    };

    let err = resolve_read_text_request(&M1State::default(), &params, None)
        .err()
        .context("disjoint client region must fail")?;
    ensure!(
        error_code(&err) == Some(error_codes::CAPTURE_TARGET_INVALID),
        "expected CAPTURE_TARGET_INVALID for disjoint client region, got: {}",
        err.message
    );
    Ok(())
}
