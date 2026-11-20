//! Regression tests for #133 driving the hidden-desktop regionless `read_text`
//! caller plan (`hidden_desktop_read_text_capture_plan` +
//! `hidden_desktop_read_text_request_region`), `SynapseService::read_text_request_with_captured_bitmap`,
//! `SynapseService::read_text_hidden_desktop`, and `resolve_read_text_request`.

use anyhow::{Context, ensure};
use synapse_core::{ElementId, OcrBackend, Rect, error_codes};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SW_SHOWNOACTIVATE, ShowWindow, WS_EX_NOACTIVATE, WS_POPUP,
};
use windows::core::w;

use super::{
    CapturedOcrBitmap, ReadTextParams, hidden_desktop_read_text_capture_plan,
    hidden_desktop_read_text_request_region, mcp_error, resolve_read_text_request,
};
use crate::m1::M1State;
use crate::test_support::{PumpedWindows, from_hwnd};

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn must_pass_regionless_hidden_desktop_plan_and_ocr_pipeline_normalize_to_dwm_bitmap_extent()
-> anyhow::Result<()> {
    let dir = tempfile::TempDir::new().context("tempdir")?;
    let service = super::tests::service_with_temp_db(dir.path())?;
    let plan = hidden_desktop_read_text_capture_plan(None);
    ensure!(
        plan.capture_region.is_none() && !plan.client_region && plan.whole_window,
        "#133: regionless hidden-desktop plan must request whole-window DWM capture: {plan:?}"
    );

    let dwm_frame_anchor = Rect {
        x: 8,
        y: 31,
        w: 1502,
        h: 1039,
    };
    let request = crate::m1::ResolvedReadTextRequest {
        region: hidden_desktop_read_text_request_region(&plan, None),
        capture_source: crate::m1::ReadTextCaptureSource::WholeWindow { hwnd: 0x1330 },
        requested_backend: OcrBackend::Auto,
        effective_backend: OcrBackend::Winrt,
        lang_hint: None,
        synthetic: true,
    };
    let captured = CapturedOcrBitmap {
        bitmap: synapse_capture::CapturedBgraBitmap {
            region: dwm_frame_anchor,
            width: 1502,
            height: 1039,
            bytes: Vec::new(),
        },
        capture_source: "whole_window",
        capture_backend: "printwindow",
        capture_hwnd: Some(0x1330),
        capture_region: dwm_frame_anchor,
    };

    let result = service
        .read_text_request_with_captured_bitmap(request, captured)
        .map_err(|error| {
            anyhow::anyhow!("read_text_request_with_captured_bitmap: {}", error.message)
        })?;
    ensure!(
        result.region
            == Rect {
                x: 0,
                y: 0,
                w: 1502,
                h: 1039
            },
        "#133: whole-window OCR result region must match 1502x1039 DWM bitmap extent, got {:?}",
        result.region
    );
    let first_word = result.words.first().context("synthetic OCR word")?;
    ensure!(
        first_word.bbox
            == Rect {
                x: 4,
                y: 4,
                w: 72,
                h: 18
            },
        "#133: whole-window word bbox must be anchored at bitmap (0,0), got {:?}",
        first_word.bbox
    );
    Ok(())
}

#[test]
fn must_pass_resolve_read_text_request_on_live_window_without_region_selects_whole_window()
-> anyhow::Result<()> {
    // A message-pumping UI thread owns the window, so a concurrent UIA query
    // that reaches it is answered; dropping `window` destroys it there.
    let window = PumpedWindows::spawn(|| {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("Static"),
                w!("synapse-133-live-window"),
                WS_POPUP,
                40,
                40,
                240,
                120,
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
    let hwnd_i64 = window.hwnd(0);
    let params = ReadTextParams {
        region: None,
        element_id: None,
        window_hwnd: Some(hwnd_i64),
        backend: OcrBackend::Auto,
        lang_hint: None,
    };
    let resolved = resolve_read_text_request(&M1State::default(), &params, None);
    drop(window);

    let resolved = resolved
        .map_err(|error| anyhow::anyhow!("resolve_read_text_request: {}", error.message))?;
    ensure!(
        resolved.capture_source == crate::m1::ReadTextCaptureSource::WholeWindow { hwnd: hwnd_i64 }
            && resolved.region
                == Rect {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0
                },
        "#133: regionless window read_text must resolve to WholeWindow, got {resolved:?}"
    );
    Ok(())
}

#[test]
fn must_fail_read_text_hidden_desktop_refuses_element_id_and_empty_whole_window_bitmap()
-> anyhow::Result<()> {
    let dir = tempfile::TempDir::new().context("tempdir")?;
    let service = super::tests::service_with_temp_db(dir.path())?;
    let fallback = mcp_error(error_codes::OCR_NO_TEXT, "element_id fallback sentinel");
    let params = ReadTextParams {
        region: None,
        element_id: Some(ElementId::parse("0x1234:2a").context("parse ElementId")?),
        window_hwnd: Some(0x1234),
        backend: OcrBackend::Auto,
        lang_hint: None,
    };
    let err = service
        .read_text_hidden_desktop(&params, "session-133", 0x1234, fallback)
        .err()
        .context("element_id on hidden desktop must fail with original_error")?;
    ensure!(
        error_code(&err) == Some(error_codes::OCR_NO_TEXT)
            && err.message.contains("element_id fallback sentinel"),
        "unexpected error for element_id on hidden desktop: {}",
        err.message
    );

    let empty_request = crate::m1::ResolvedReadTextRequest {
        region: Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        capture_source: crate::m1::ReadTextCaptureSource::WholeWindow { hwnd: 0x1330 },
        requested_backend: OcrBackend::Auto,
        effective_backend: OcrBackend::Winrt,
        lang_hint: None,
        synthetic: true,
    };
    let empty_bitmap = CapturedOcrBitmap {
        bitmap: synapse_capture::CapturedBgraBitmap {
            region: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            width: 0,
            height: 0,
            bytes: Vec::new(),
        },
        capture_source: "whole_window",
        capture_backend: "printwindow",
        capture_hwnd: Some(0x1330),
        capture_region: Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
    };
    let empty_err = service
        .read_text_request_with_captured_bitmap(empty_request, empty_bitmap)
        .err()
        .context("0x0 whole-window bitmap must fail")?;
    ensure!(
        error_code(&empty_err) == Some(error_codes::OCR_NO_TEXT),
        "expected OCR_NO_TEXT for 0x0 bitmap, got: {}",
        empty_err.message
    );
    Ok(())
}
