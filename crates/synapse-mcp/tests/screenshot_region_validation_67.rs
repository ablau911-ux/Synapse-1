//! #67: an invalid client region on an ordinary window target must fail as
//! `CAPTURE_TARGET_INVALID` with the region cause, before any hidden-desktop
//! fallback, and must create no artifact. A valid region still writes the
//! exact client pixels (contract: `server/m1_tools.rs` `capture_screenshot`,
//! window branch: `client_region_to_window_region` errors propagate with `?`
//! before `capture_hidden_desktop_screenshot_to_file`).
//!
//! The session is fresh (no hidden desktop), the target is a test-owned
//! solid-colour window, and every result is read back from disk.

#![cfg(windows)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use anyhow::{Context, ensure};
use g6a_support::error_code;
use g6a_window::{BLUE, PaintedWindow, WindowSpec, classify_rgb};
use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};

fn capture_args(path: &std::path::Path, hwnd: i64, region: Value) -> Value {
    json!({
        "operation": "capture", "path": path.to_string_lossy(),
        "window_hwnd": hwnd, "region": region
    })
}

/// Asserts the region is refused as `CAPTURE_TARGET_INVALID`, never as a
/// hidden-desktop error, and that no file was written.
async fn assert_region_refused(
    session: &mut HttpMcpSession,
    path: &std::path::Path,
    hwnd: i64,
    region: Value,
) -> anyhow::Result<()> {
    ensure!(!path.exists(), "precondition: {} absent", path.display());
    let error = session
        .tools_call_error("screenshot", capture_args(path, hwnd, region.clone()))
        .await?;
    ensure!(
        error_code(&error) == synapse_core::error_codes::CAPTURE_TARGET_INVALID,
        "#67: region {region} must be CAPTURE_TARGET_INVALID: {error}"
    );
    let message = error["message"].as_str().unwrap_or_default();
    ensure!(
        !message.contains("hidden desktop"),
        "#67: region error must not be masked by the hidden-desktop fallback: {message}"
    );
    ensure!(
        !path.exists(),
        "#67: refused region wrote {}",
        path.display()
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_zero_width_region_is_target_invalid_without_artifact() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(240, 160, BLUE));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-67-w0", &[]).await?;
    let path = daemon.home().join("zero-width-67.png");
    let region = json!({"x": 0, "y": 0, "w": 0, "h": 20});
    assert_region_refused(&mut session, &path, window.hwnd(), region).await?;
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_zero_height_region_is_target_invalid_without_artifact() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(240, 160, BLUE));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-67-h0", &[]).await?;
    let path = daemon.home().join("zero-height-67.png");
    let region = json!({"x": 0, "y": 0, "w": 20, "h": 0});
    assert_region_refused(&mut session, &path, window.hwnd(), region).await?;
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_out_of_bounds_region_is_target_invalid_without_artifact() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(240, 160, BLUE));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-67-oob", &[]).await?;
    let path = daemon.home().join("out-of-bounds-67.png");
    let region = json!({"x": 99_999, "y": 0, "w": 20, "h": 20});
    assert_region_refused(&mut session, &path, window.hwnd(), region).await?;
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_valid_region_writes_exact_client_pixels() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(240, 160, BLUE));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-67-ok", &[]).await?;
    let path = daemon.home().join("valid-67.png");
    let region = json!({"x": 10, "y": 10, "w": 40, "h": 30});
    let response = session
        .tools_call("screenshot", capture_args(&path, window.hwnd(), region))
        .await?;
    let capture = &response["structuredContent"]["capture"];
    ensure!(
        capture["width"] == 40 && capture["height"] == 30,
        "valid region dimensions mismatch: {response}"
    );
    let bytes =
        std::fs::read(&path).with_context(|| format!("no artifact at {}", path.display()))?;
    let image = image::load_from_memory(&bytes)?.to_rgb8();
    ensure!(
        image.dimensions() == (40, 30),
        "decoded {:?}",
        image.dimensions()
    );
    ensure!(
        image
            .pixels()
            .all(|p| classify_rgb([p[0], p[1], p[2]]) == Some(BLUE)),
        "every pixel of the client region must be the window's BLUE"
    );
    drop(daemon.stop().await?);
    Ok(())
}
