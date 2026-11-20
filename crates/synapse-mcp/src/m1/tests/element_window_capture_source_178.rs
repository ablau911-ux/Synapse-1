//! Regression tests for #178 driving `element_window_capture_source` and
//! `resolve_read_text_request` against a real Win32 window so `element_id`
//! OCR is proven to select `ReadTextCaptureSource::Window` (never `Screen`)
//! and to fail closed when the element is outside the window or the window
//! is minimized/destroyed.

use anyhow::{Context, ensure};
use synapse_core::{ElementId, OcrBackend, Rect, error_codes};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, ShowWindow, WS_EX_NOACTIVATE,
    WS_MINIMIZEBOX, WS_POPUP,
};
use windows::core::w;

use super::{
    M1State, ReadTextCaptureSource, ReadTextParams, element_window_capture_source,
    resolve_read_text_request,
};
use crate::test_support::{PumpedWindows, from_hwnd, to_hwnd};

/// Popup owned by a message-pumping UI thread: UIA reads of it are answered
/// while the test thread blocks inside `snapshot_window_from_hwnd`.
struct LivePopupWindow {
    ui: PumpedWindows,
}

impl LivePopupWindow {
    fn open(x: i32, y: i32, w: i32, h: i32) -> anyhow::Result<Self> {
        let ui = PumpedWindows::spawn(move || {
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("Static"),
                    w!("synapse-178-element-window"),
                    WS_POPUP | WS_MINIMIZEBOX,
                    x,
                    y,
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

    fn minimize(&self) -> anyhow::Result<()> {
        let hwnd = self.hwnd_i64();
        self.ui.run(move || {
            let _ = unsafe { ShowWindow(to_hwnd(hwnd), SW_SHOWMINNOACTIVE) };
        })
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
fn must_pass_element_inside_live_window_resolves_window_capture_source_never_screen()
-> anyhow::Result<()> {
    let win = LivePopupWindow::open(100, 120, 320, 200)?;
    let hwnd = win.hwnd_i64();
    let frame = synapse_capture::window_capture_bounds(hwnd).context("window_capture_bounds")?;
    let element_id = ElementId::parse(&format!("{hwnd:#x}:2a")).context("ElementId::parse")?;
    let element_rect = Rect {
        x: frame.x + 25,
        y: frame.y + 30,
        w: 80,
        h: 24,
    };

    let (screen_region, source) = element_window_capture_source(&element_id, element_rect)
        .map_err(|error| anyhow::anyhow!("element_window_capture_source: {}", error.message))?;
    ensure!(
        screen_region == element_rect
            && source
                == ReadTextCaptureSource::Window {
                    hwnd,
                    window_region: Rect {
                        x: 25,
                        y: 30,
                        w: 80,
                        h: 24,
                    },
                },
        "#178: element inside window must map to Window capture source, got ({screen_region:?}, {source:?})"
    );

    let subtree =
        synapse_a11y::snapshot_window_from_hwnd(hwnd, 1).context("snapshot_window_from_hwnd")?;
    let params = ReadTextParams {
        region: None,
        element_id: Some(subtree.root),
        window_hwnd: None,
        backend: OcrBackend::Auto,
        lang_hint: None,
    };
    let resolved = resolve_read_text_request(&M1State::default(), &params, None)
        .map_err(|error| anyhow::anyhow!("resolve_read_text_request: {}", error.message))?;
    ensure!(
        matches!(
            resolved.capture_source,
            ReadTextCaptureSource::Window {
                hwnd: resolved_hwnd,
                ..
            } if resolved_hwnd == hwnd
        ),
        "#178: resolve_read_text_request(element_id) must select Window capture source, got {:?}",
        resolved.capture_source
    );
    Ok(())
}

#[test]
fn must_fail_element_rect_outside_live_window_frame_refuses_screen_fallback() -> anyhow::Result<()>
{
    let win = LivePopupWindow::open(100, 120, 320, 200)?;
    let hwnd = win.hwnd_i64();
    let frame = synapse_capture::window_capture_bounds(hwnd).context("window_capture_bounds")?;
    let element_id = ElementId::parse(&format!("{hwnd:#x}:2a")).context("ElementId::parse")?;
    let outside_rect = Rect {
        x: frame.x + frame.w + 40,
        y: frame.y,
        w: 60,
        h: 20,
    };

    let err = element_window_capture_source(&element_id, outside_rect)
        .err()
        .context("outside element rect must be refused")?;
    ensure!(
        error_code(&err) == Some(error_codes::OCR_NO_TEXT)
            && err.message.contains("screen capture is refused"),
        "#178: outside element must refuse screen fallback with OCR_NO_TEXT, got: {}",
        err.message
    );
    Ok(())
}

#[test]
fn must_fail_minimized_or_destroyed_element_window_is_refused() -> anyhow::Result<()> {
    let win = LivePopupWindow::open(100, 120, 320, 200)?;
    let hwnd = win.hwnd_i64();
    let element_id = ElementId::parse(&format!("{hwnd:#x}:2a")).context("ElementId::parse")?;
    let rect = Rect {
        x: 110,
        y: 130,
        w: 50,
        h: 20,
    };

    win.minimize()?;
    let min_err = element_window_capture_source(&element_id, rect)
        .err()
        .context("minimized window must be refused")?;
    ensure!(
        error_code(&min_err) == Some(error_codes::A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE),
        "expected A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE, got: {}",
        min_err.message
    );

    drop(win);
    let dead_err = element_window_capture_source(&element_id, rect)
        .err()
        .context("destroyed window must be refused")?;
    ensure!(
        error_code(&dead_err) == Some(error_codes::TARGET_WINDOW_NOT_FOUND),
        "expected TARGET_WINDOW_NOT_FOUND, got: {}",
        dead_err.message
    );
    Ok(())
}
