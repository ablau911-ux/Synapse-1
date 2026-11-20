//! End-to-end HTTP integration tests for #117: drives the real `browser_capture`
//! MCP facade to verify that `operation=screenshot` with an explicit `window_hwnd`
//! (and no `cdp_target_id` or page-only options) captures the window passively
//! without requiring tab ownership, whereas `cdp_target_id` preserves tab-ownership
//! enforcement and invalid formats/HWNDs fail closed.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use anyhow::{Context, ensure};
use g6a_window::{PaintedWindow, RED, WindowSpec};
use serde_json::json;
use support::HttpDaemon;

#[tokio::test]
async fn must_pass_browser_capture_explicit_hwnd_writes_window_capture_without_tab_ownership()
-> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(240, 180, RED));
    let rect = window.rect();
    ensure!(
        (rect.right - rect.left, rect.bottom - rect.top) == (240, 180),
        "precondition: test-owned window is 240x180 physical pixels: {rect:?}"
    );
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6c-117-window", &[]).await?;
    let out_path = daemon.home().join("window-117.png");

    let response = session
        .tools_call(
            "browser_capture",
            json!({
                "operation": "screenshot",
                "screenshot": {
                    "path": out_path.to_string_lossy(),
                    "window_hwnd": window.hwnd(),
                    "overwrite": true
                }
            }),
        )
        .await?;
    let structured = &response["structuredContent"];
    ensure!(
        structured["screenshot"].is_null() && structured["window_capture"].is_object(),
        "#117: browser_capture with explicit window_hwnd must populate window_capture: {response}"
    );
    let decoded = image::open(&out_path).context("decode written PNG from disk")?;
    ensure!(
        decoded.width() == 240 && decoded.height() == 180,
        "decoded PNG dimensions {}x{} must match 240x180 window",
        decoded.width(),
        decoded.height()
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_browser_capture_with_cdp_target_id_keeps_tab_ownership_gate()
-> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(240, 180, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6c-117-tab-gate", &[]).await?;
    let out_path = daemon.home().join("unowned-tab-117.png");

    let error = session
        .tools_call_error(
            "browser_capture",
            json!({
                "operation": "screenshot",
                "screenshot": {
                    "path": out_path.to_string_lossy(),
                    "window_hwnd": window.hwnd(),
                    "cdp_target_id": "unowned-tab-target-117",
                    "overwrite": true
                }
            }),
        )
        .await?;
    ensure!(
        error["data"]["code"] == json!("ACTION_TARGET_INVALID") && !out_path.exists(),
        "#117 contract: unowned cdp_target_id must return ACTION_TARGET_INVALID (ownership-gated, issue thread) and write no file: {error}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_browser_capture_window_route_rejects_format_conflict_and_hwnd_zero()
-> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(240, 180, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6c-117-invalid", &[]).await?;
    let conflict_path = daemon.home().join("conflict-117.png");

    let conflict = session
        .tools_call_error(
            "browser_capture",
            json!({
                "operation": "screenshot",
                "screenshot": {
                    "path": conflict_path.to_string_lossy(),
                    "window_hwnd": window.hwnd(),
                    "format": "jpeg",
                    "overwrite": true
                }
            }),
        )
        .await?;
    ensure!(
        conflict["data"]["code"] == json!(synapse_core::error_codes::TOOL_PARAMS_INVALID)
            && !conflict_path.exists(),
        "format=jpeg on .png path must fail with TOOL_PARAMS_INVALID: {conflict}"
    );

    let zero_path = daemon.home().join("zero-117.png");
    let zero_err = session
        .tools_call_error(
            "browser_capture",
            json!({
                "operation": "screenshot",
                "screenshot": {
                    "path": zero_path.to_string_lossy(),
                    "window_hwnd": 0,
                    "overwrite": true
                }
            }),
        )
        .await?;
    ensure!(
        zero_err["data"]["code"] == json!(synapse_core::error_codes::CAPTURE_TARGET_INVALID)
            && !zero_path.exists(),
        "window_hwnd=0 must route to window capture and fail with CAPTURE_TARGET_INVALID: {zero_err}"
    );
    drop(daemon.stop().await?);
    Ok(())
}
