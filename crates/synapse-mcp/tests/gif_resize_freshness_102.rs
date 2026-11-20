//! #102: a GIF of a live target must record the target's real rendered states
//! when the target changes (colour and size) during the recording; a full
//! frame count of one stale bitmap is a failure, and dropped frames are never
//! silent (`frames_captured` vs `frames_requested`).
//!
//! The target is a test-owned, never-activated solid-colour window whose state
//! timeline is driven by the test: RED 400x300 -> GREEN 300x200 -> BLUE 400x300.
//! The timeline is anchored on the daemon tool-ledger `started` row (physical
//! SoT that recording began), and each step is confirmed by the window's own
//! paint readback and `GetWindowRect` before the next. The Tauri/WebView
//! compositor case of the original report still needs a live VBV; this is the
//! deterministic Win32 regression.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use std::time::Duration;

use anyhow::ensure;
use g6a_support::{centre_rgb, error_code, gif_frames, wait_gif_frame_captured, wait_tool_started};
use g6a_window::{BLUE, GREEN, PaintedWindow, RED, WindowSpec, classify_rgb};
use serde_json::{Value, json};
use support::HttpDaemon;
use tokio::time::{Instant, sleep_until};

const STEP: Duration = Duration::from_secs(5);

fn gif_args(path: &std::path::Path, hwnd: i64, duration_ms: u64) -> Value {
    json!({
        "operation": "gif", "path": path.to_string_lossy(), "window_hwnd": hwnd,
        "duration_ms": duration_ms, "interval_ms": 200, "max_long_edge": 0
    })
}

/// Changes the window to `color` at `w`x`h` and reads both back.
fn step(window: &PaintedWindow, color: u32, w: i32, h: i32) -> anyhow::Result<()> {
    let rect = window.resize(w, h);
    ensure!(
        rect.right - rect.left == w && rect.bottom - rect.top == h,
        "GetWindowRect readback {rect:?} != {w}x{h}"
    );
    window.set_color(color);
    ensure!(
        window.wait_painted(color, STEP),
        "window never painted {color:#08x}"
    );
    Ok(())
}

/// Classified centre colour of every decoded frame.
fn frame_colours(path: &std::path::Path) -> anyhow::Result<Vec<Option<u32>>> {
    Ok(gif_frames(path)?
        .iter()
        .map(|f| classify_rgb(centre_rgb(f)))
        .collect())
}

#[tokio::test]
async fn must_pass_gif_records_each_colour_and_size_state_in_order() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(400, 300, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-102", &[]).await?;
    let path = daemon.home().join("transition-102.gif");
    let args = gif_args(&path, window.hwnd(), 2400);
    let task = tokio::spawn(async move { session.tools_call("screenshot", args).await });
    ensure!(
        wait_tool_started(daemon.db_path(), "screenshot", 1, STEP).await,
        "GIF call never reached the daemon tool ledger"
    );
    let t0 = Instant::now();
    sleep_until(t0 + Duration::from_millis(700)).await;
    step(&window, GREEN, 300, 200)?;
    sleep_until(t0 + Duration::from_millis(1500)).await;
    step(&window, BLUE, 400, 300)?;
    let response = task.await??;

    let colours = frame_colours(&path)?;
    let reported = response["structuredContent"]["gif"]["frames_captured"].as_u64();
    ensure!(
        reported == Some(colours.len() as u64),
        "frame count mismatch: {response}"
    );
    ensure!(
        colours.first() == Some(&Some(RED)),
        "first frame must be RED: {colours:?}"
    );
    ensure!(
        colours.contains(&Some(GREEN)),
        "#102: no GREEN (resized) frame: {colours:?}"
    );
    ensure!(
        colours.last() == Some(&Some(BLUE)),
        "#102: last frame must be BLUE: {colours:?}"
    );
    let order: Vec<u32> = colours.iter().flatten().copied().collect();
    let rank = |c: u32| [RED, GREEN, BLUE].iter().position(|x| *x == c).unwrap_or(9);
    ensure!(
        order.windows(2).all(|pair| rank(pair[0]) <= rank(pair[1])),
        "#102: frames must follow the real RED->GREEN->BLUE order: {colours:?}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_static_window_gif_frames_all_show_the_same_state() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, GREEN));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-102-static", &[]).await?;
    let path = daemon.home().join("static-102.gif");
    let response = session
        .tools_call("screenshot", gif_args(&path, window.hwnd(), 1000))
        .await?;
    let colours = frame_colours(&path)?;
    ensure!(
        colours.len() >= 2,
        "static GIF must have frames: {response}"
    );
    ensure!(
        colours.iter().all(|c| *c == Some(GREEN)),
        "static control must stay GREEN in every frame: {colours:?}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_target_lost_mid_recording_reports_an_honest_partial_count() -> anyhow::Result<()>
{
    let mut window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-102-lost", &[]).await?;
    ensure!(
        window.wait_captured_color(RED, STEP),
        "precondition: WGC sees RED fixture"
    );
    let path = daemon.home().join("lost-102.gif");
    let args = gif_args(&path, window.hwnd(), 10_000);
    let task = tokio::spawn(async move { session.tools_call("screenshot", args).await });
    ensure!(
        wait_tool_started(daemon.db_path(), "screenshot", 1, STEP).await,
        "GIF call never reached the daemon tool ledger"
    );
    ensure!(
        wait_gif_frame_captured(&daemon.home().join("logs"), window.hwnd(), STEP).await,
        "GIF worker never acquired a verified frame before target loss"
    );
    window.destroy();
    ensure!(
        !window.is_alive(),
        "precondition: target destroyed mid-recording"
    );
    let response = task.await??;

    let gif = &response["structuredContent"]["gif"];
    let captured = gif["frames_captured"].as_u64().unwrap_or_default();
    let requested = gif["frames_requested"].as_u64().unwrap_or_default();
    ensure!(
        requested == 50,
        "10000/200 must request 50 frames: {response}"
    );
    ensure!(
        (1..requested).contains(&captured),
        "a lost target must be reported as a partial count, got {captured}/{requested}"
    );
    let colours = frame_colours(&path)?;
    ensure!(
        colours.len() as u64 == captured,
        "decoded {} != {captured}",
        colours.len()
    );
    ensure!(
        colours.iter().all(|c| *c == Some(RED)),
        "only real frames: {colours:?}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_gif_of_a_destroyed_window_publishes_nothing() -> anyhow::Result<()> {
    let mut window = PaintedWindow::open(WindowSpec::visible(200, 150, RED));
    let stale_hwnd = window.hwnd();
    window.destroy();
    ensure!(
        !window.is_alive(),
        "precondition: HWND {stale_hwnd:#x} is gone"
    );
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-102-gone", &[]).await?;
    let path = daemon.home().join("gone-102.gif");
    let error = session
        .tools_call_error("screenshot", gif_args(&path, stale_hwnd, 400))
        .await?;
    ensure!(
        error_code(&error) == synapse_core::error_codes::ACTION_NO_OBSERVED_DELTA,
        "zero frames must be ACTION_NO_OBSERVED_DELTA: {error}"
    );
    ensure!(
        !path.exists(),
        "no artifact may be published for zero frames"
    );
    drop(daemon.stop().await?);
    Ok(())
}
