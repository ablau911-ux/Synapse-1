//! #122 (re-review): when two sessions' `livecapture start` share one capture
//! (the second adopts it) and the capture worker then ENDS before any frame,
//! both failed starts must tear the dead capture down (a joiner must not
//! suppress cleanup), and `status` must report the real state, never
//! `running` for a dead worker. A healthy adopted capture still survives (#110).
//!
//! Contract: `server/livecapture.rs` failed-start cleanup +
//! `livecapture_ownership::failed_start_should_stop`; `m1.rs`
//! `capture_runtime_readback` (`inactive` with no capture, `capture_status_label`).
//! Driven through the real public `livecapture` facade over two HTTP sessions;
//! the worker death is caused physically by destroying the test-owned target
//! window (WGC item closed). Sources of truth: `livecapture status` from a third
//! session and the daemon tool ledger.

#![cfg(windows)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use std::time::{Duration, Instant};

use anyhow::ensure;
use g6a_support::wait_session_tool_started;
use g6a_window::{PaintedWindow, RED, WindowSpec};
use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};

const STEP: Duration = Duration::from_secs(10);

async fn runtime(session: &mut HttpMcpSession) -> anyhow::Result<Value> {
    let status = session
        .tools_call("livecapture", json!({"operation": "status"}))
        .await?;
    Ok(status["structuredContent"]["runtime"].clone())
}

/// Polls `status` until `runtime.status == want` (deadline, no fixed sleep).
async fn wait_status(session: &mut HttpMcpSession, want: &str) -> anyhow::Result<Value> {
    let deadline = Instant::now() + STEP;
    loop {
        let readback = runtime(session).await?;
        if readback["status"] == json!(want) || Instant::now() >= deadline {
            return Ok(readback);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn start_args(hwnd: i64) -> Value {
    json!({"operation": "start", "window_hwnd": hwnd})
}

#[tokio::test]
async fn must_fail_dead_adopted_worker_is_cleaned_up_and_never_reported_running()
-> anyhow::Result<()> {
    let mut window = PaintedWindow::open(WindowSpec::minimized(RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut reader = daemon.session("g6a-122-reader", &[]).await?;
    ensure!(
        runtime(&mut reader).await?["status"] == json!("inactive"),
        "precondition: idle"
    );
    let mut first = daemon.session("g6a-122-a", &[]).await?;
    let mut second = daemon.session("g6a-122-b", &[]).await?;
    first.tools_list().await?;
    second.tools_list().await?;

    let (a_id, b_id) = (
        first.session_id().to_owned(),
        second.session_id().to_owned(),
    );
    let hwnd = window.hwnd();
    let a = tokio::spawn(async move {
        first
            .tools_call_error("livecapture", start_args(hwnd))
            .await
    });
    let db = daemon.db_path();
    ensure!(
        wait_session_tool_started(db, "livecapture", &a_id, STEP).await,
        "A not started"
    );
    let running = wait_status(&mut reader, "running").await?;
    ensure!(
        running["status"] == json!("running") && running["frames_captured"] == json!(0),
        "precondition: A's worker runs and has no frame yet: {running}"
    );
    let b = tokio::spawn(async move {
        second
            .tools_call_error("livecapture", start_args(hwnd))
            .await
    });
    ensure!(
        wait_session_tool_started(db, "livecapture", &b_id, STEP).await,
        "B not started"
    );

    window.destroy();
    ensure!(!window.is_alive(), "precondition: capture target destroyed");
    let (a_error, b_error) = (a.await??, b.await??);
    for error in [&a_error, &b_error] {
        // #122 original symptom: the dead worker was masked as a frame timeout.
        let message = error["message"].as_str().unwrap_or_default();
        ensure!(
            !message.contains("after_frame_timeout") && !g6a_support::error_code(error).is_empty(),
            "#122: a start on a dead worker must report the worker end, not a timeout: {error}"
        );
    }
    let after = wait_status(&mut reader, "inactive").await?;
    ensure!(
        after["status"] == json!("inactive"),
        "#122: a dead adopted worker must be torn down, status was {after}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_healthy_adopted_capture_survives_and_reports_running() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut owner = daemon.session("g6a-122-owner", &[]).await?;
    let mut adopter = daemon.session("g6a-122-adopter", &[]).await?;
    let mut reader = daemon.session("g6a-122-reader2", &[]).await?;

    let first = owner
        .tools_call("livecapture", start_args(window.hwnd()))
        .await?;
    ensure!(
        first["structuredContent"]["frame_count"] == json!(1),
        "start frame: {first}"
    );
    let second = adopter
        .tools_call("livecapture", start_args(window.hwnd()))
        .await?;
    ensure!(
        second["structuredContent"]["frame_count"] == json!(1),
        "adopt frame: {second}"
    );
    let generation = first["structuredContent"]["runtime"]["generation"].clone();
    ensure!(
        second["structuredContent"]["runtime"]["generation"] == generation,
        "the adopter must share the running capture, not restart it"
    );
    let live = runtime(&mut reader).await?;
    ensure!(
        live["status"] == json!("running"),
        "healthy capture must run: {live}"
    );

    let stopped = owner
        .tools_call("livecapture", json!({"operation": "stop"}))
        .await?;
    ensure!(
        stopped["structuredContent"]["stopped"] == json!(true),
        "owner stop: {stopped}"
    );
    ensure!(
        runtime(&mut reader).await?["status"] == json!("inactive"),
        "stopped capture"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_start_on_destroyed_window_is_refused_and_leaves_no_capture() -> anyhow::Result<()>
{
    let mut window = PaintedWindow::open(WindowSpec::visible(200, 150, RED));
    let stale = window.hwnd();
    window.destroy();
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-122-stale", &[]).await?;
    let error = session
        .tools_call_error("livecapture", start_args(stale))
        .await?;
    ensure!(
        error["data"]["code"] == json!(synapse_core::error_codes::CAPTURE_TARGET_INVALID),
        "a destroyed HWND must be CAPTURE_TARGET_INVALID: {error}"
    );
    ensure!(
        runtime(&mut session).await?["status"] == json!("inactive"),
        "no capture left"
    );
    drop(daemon.stop().await?);
    Ok(())
}
