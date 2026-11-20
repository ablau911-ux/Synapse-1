//! #81: a verified coordinate click bound to a window must take its before/after
//! evidence from that window's own pixels (bounded Win32 + window capture), not
//! from a broad depth-6 UIA snapshot that a hung WebView2 provider can block for
//! the whole worker deadline before any input is sent.
//!
//! NEEDS-SEAM (fixer adds; the handler `act_click` uses it instead of its inline
//! `coordinate_window_hwnd` / `capture_coordinate_click_delta_signature` choice,
//! behaviour unchanged):
//!   #[derive(Clone, Copy, Debug, Eq, PartialEq)]
//!   enum ClickBeforeCapture {
//!       None,
//!       CoordinatePixels { hwnd: i64 },
//!       Semantic { target_window_hwnd: Option<i64> },
//!   }
//!   fn click_before_capture(params: &ActClickParams, target_window_hwnd: Option<i64>)
//!       -> ClickBeforeCapture

use serde_json::{Value, json};

use super::*;

const BOUND: i64 = 0x00C8_0A10;

fn click(target: Value, verify_delta: bool) -> ActClickParams {
    serde_json::from_value(json!({ "target": target, "verify_delta": verify_delta }))
        .expect("valid synthetic click params")
}

#[test]
fn must_pass_window_bound_point_click_captures_window_pixels_only() {
    let params = click(json!({ "x": 280, "y": 925 }), true);
    assert_eq!(
        click_before_capture(&params, Some(BOUND)),
        ClickBeforeCapture::CoordinatePixels { hwnd: BOUND }
    );
}

#[test]
fn must_pass_element_click_keeps_the_semantic_capture() {
    let params = click(json!({ "element_id": "0xc80a10:2a00000001" }), true);
    assert_eq!(
        click_before_capture(&params, Some(BOUND)),
        ClickBeforeCapture::Semantic {
            target_window_hwnd: Some(BOUND)
        }
    );
}

#[test]
fn must_fail_unbound_point_click_does_not_claim_a_window_pixel_source() {
    // No resolved window: there is no window to read pixels from, so the
    // window-pixel route must not be invented with a fake HWND.
    let params = click(json!({ "x": 280, "y": 925 }), true);
    assert_eq!(
        click_before_capture(&params, None),
        ClickBeforeCapture::Semantic {
            target_window_hwnd: None
        }
    );
}

#[test]
fn must_fail_unverified_click_captures_nothing() {
    let params = click(json!({ "x": 280, "y": 925 }), false);
    assert_eq!(
        click_before_capture(&params, Some(BOUND)),
        ClickBeforeCapture::None
    );
}

/// Handler wiring: `act_click` takes its before-capture decision from the seam.
#[test]
fn must_pass_act_click_uses_the_capture_decision() {
    let source = include_str!("../m2_tools.rs");
    let start = source.find("pub async fn act_click(").expect("handler");
    let body = &source[start..];
    let body = &body[..body.find("\n    #[tool(").unwrap_or(body.len())];
    assert!(
        body.contains("click_before_capture("),
        "act_click must use the seam"
    );
}

#[cfg(windows)]
mod live_window {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, SW_SHOWNOACTIVATE, ShowWindow, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_POPUP,
    };
    use windows::core::w;

    use crate::server::SynapseService;
    use crate::test_support::{PumpedWindows, from_hwnd};

    /// A test-owned, never-activated, off-screen window on a message-pumping
    /// UI thread (a concurrent UIA query that reaches it is answered);
    /// destroyed on drop.
    struct OwnWindow(PumpedWindows);

    impl OwnWindow {
        fn hwnd(&self) -> i64 {
            self.0.hwnd(0)
        }
    }

    fn own_window() -> OwnWindow {
        let ui = PumpedWindows::spawn(|| {
            // SAFETY: STATIC is a system class; no Rust pointer is handed to Windows.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("synapse #81 coordinate capture"),
                    WS_POPUP,
                    -16_000,
                    -16_000,
                    200,
                    120,
                    None,
                    None,
                    None,
                    None,
                )
            }?;
            // SAFETY: live handle owned by this thread; shown without activation.
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            Ok(vec![from_hwnd(hwnd)])
        });
        OwnWindow(ui.unwrap_or_else(|error| panic!("CreateWindowExW failed: {error:#}")))
    }

    fn service_without_semantic_perception() -> (tempfile::TempDir, SynapseService) {
        let root = tempfile::tempdir().expect("temp dir");
        let service = SynapseService::try_new_isolated_for_tests(root.path()).expect("service");
        let mut state = service.m1_state.lock().expect("m1 state");
        state.force_no_perception = true;
        // Precondition readback: every semantic observation now fails.
        let observed = crate::m1::current_input(&state, 6).expect_err("perception is off");
        assert_eq!(
            observed
                .data
                .as_ref()
                .and_then(|data| data["code"].as_str()),
            Some(synapse_core::error_codes::OBSERVE_NO_PERCEPTION_AVAILABLE)
        );
        drop(state);
        (root, service)
    }

    #[test]
    fn must_pass_window_pixel_capture_needs_no_semantic_perception() {
        let window = own_window();
        let hwnd = window.hwnd();
        let (_root, service) = service_without_semantic_perception();
        let signature = service
            .capture_coordinate_click_delta_signature(hwnd)
            .expect("window-bound capture must not depend on UIA/observation");
        assert_eq!(signature.foreground_hwnd, hwnd);
        assert_eq!(signature.foreground_pid, std::process::id());
        assert_eq!(signature.web_path.as_deref(), Some("pixel_only_coordinate"));
        assert_eq!(signature.element_count, 0);
        assert_eq!(signature.focused_element_id, None);
    }

    #[test]
    fn must_fail_dead_window_capture_is_refused_with_no_foreground() {
        let (_root, service) = service_without_semantic_perception();
        let dead = {
            let window = own_window();
            window.hwnd()
        };
        let error = service
            .capture_coordinate_click_delta_signature(dead)
            .expect_err("a destroyed window has no pixels to verify against");
        // #315: a closed target window is TARGET_WINDOW_NOT_FOUND, not
        // A11Y_NO_FOREGROUND.
        assert_eq!(
            error.data.as_ref().and_then(|data| data["code"].as_str()),
            Some(synapse_core::error_codes::TARGET_WINDOW_NOT_FOUND)
        );
    }
}
