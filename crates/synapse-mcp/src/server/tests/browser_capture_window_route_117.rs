//! #117: browser_capture with an explicit window_hwnd (and no exact tab) must
//! route to the passive explicit-HWND window capture, not tab ownership.

use super::{explicit_window_capture_params, window_route_format_conflict};
use crate::m1::BrowserScreenshotParams;

fn params(value: serde_json::Value) -> BrowserScreenshotParams {
    match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => panic!("test params must deserialize: {error}"),
    }
}

#[test]
fn must_pass_explicit_hwnd_without_tab_routes_to_window_capture() {
    let routed = explicit_window_capture_params(&params(serde_json::json!({
        "path": "C:\\tmp\\w.png",
        "window_hwnd": 6_226_978,
        "overwrite": true,
        "max_long_edge": 1200
    })));
    let Some(capture) = routed else {
        panic!("explicit window_hwnd must select the window route");
    };
    assert_eq!(capture.window_hwnd, Some(6_226_978));
    assert_eq!(capture.path, "C:\\tmp\\w.png");
    assert!(capture.overwrite);
    assert_eq!(capture.max_long_edge, Some(1200));
    assert!(capture.region.is_none());
}

#[test]
fn must_fail_hwnd_zero_still_routes_so_capture_fails_closed_on_hwnd() {
    let routed = explicit_window_capture_params(&params(serde_json::json!({
        "path": "C:\\tmp\\w.png",
        "window_hwnd": 0
    })));
    assert_eq!(routed.map(|c| c.window_hwnd), Some(Some(0)));
}

#[test]
fn must_pass_exact_tab_capture_keeps_tab_ownership_route() {
    let routed = explicit_window_capture_params(&params(serde_json::json!({
        "path": "C:\\tmp\\w.png",
        "window_hwnd": 42,
        "cdp_target_id": "chrome-tab:7"
    })));
    assert!(routed.is_none());
}

#[test]
fn must_pass_no_hwnd_keeps_session_tab_route() {
    let routed = explicit_window_capture_params(&params(serde_json::json!({
        "path": "C:\\tmp\\w.png"
    })));
    assert!(routed.is_none());
}

#[test]
fn must_pass_page_only_options_keep_tab_route() {
    for extra in [
        serde_json::json!({"scope": "full_page"}),
        serde_json::json!({"element_id": "e1", "scope": "element"}),
        serde_json::json!({"omit_background": true}),
        serde_json::json!({"masks": [{"selector": "#x"}]}),
        serde_json::json!({"quality": 50}),
    ] {
        let mut value = serde_json::json!({"path": "C:\\tmp\\w.png", "window_hwnd": 42});
        if let (Some(base), Some(add)) = (value.as_object_mut(), extra.as_object()) {
            base.extend(add.clone());
        }
        assert!(
            explicit_window_capture_params(&params(value.clone())).is_none(),
            "{value}"
        );
    }
}

#[test]
fn must_pass_window_route_format_matching_extension_is_honored() {
    for (path, format) in [("C:/tmp/w.png", "png"), ("C:/tmp/w.jpg", "jpeg")] {
        let p = params(serde_json::json!({"path": path, "window_hwnd": 42, "format": format}));
        assert!(explicit_window_capture_params(&p).is_some(), "{path}");
        assert!(window_route_format_conflict(&p).is_none(), "{path}");
    }
}

#[test]
fn must_fail_window_route_format_conflicting_with_extension_is_rejected() {
    let p = params(serde_json::json!({
        "path": "C:/tmp/w.png", "window_hwnd": 42, "format": "jpeg"
    }));
    let Some(error) = window_route_format_conflict(&p) else {
        panic!("format=jpeg with .png path must be rejected on the window route");
    };
    let code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(
        code,
        Some(synapse_core::error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
    assert!(
        error
            .message
            .contains("conflicts with path extension (implies Png)"),
        "the refusal must name the format conflict: {}",
        error.message
    );
}
