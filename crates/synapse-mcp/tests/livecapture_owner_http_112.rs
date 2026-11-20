//! End-to-end HTTP integration tests for #112: `livecapture` `stop` and
//! retargeting `start` enforce session ownership through the live daemon,
//! `status` reports the owning session, and owner `stop` releases the capture.

#![cfg(windows)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use anyhow::ensure;
use g6a_window::{BLUE, PaintedWindow, RED, WindowSpec};
use serde_json::{Value, json};
use support::HttpDaemon;

fn start_args(hwnd: i64) -> Value {
    json!({"operation": "start", "window_hwnd": hwnd})
}

#[tokio::test]
async fn must_fail_non_owner_stop_is_refused_and_capture_stays_running() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut owner = daemon.session("g6c-112-owner", &[]).await?;
    let mut intruder = daemon.session("g6c-112-intruder", &[]).await?;
    let mut reader = daemon.session("g6c-112-reader", &[]).await?;

    let started = owner
        .tools_call("livecapture", start_args(window.hwnd()))
        .await?;
    let capture_id = started["structuredContent"]["next_cursor"]["capture_id"].clone();
    let status_before = reader
        .tools_call("livecapture", json!({"operation": "status"}))
        .await?;
    ensure!(
        status_before["structuredContent"]["owner"]["owner_session_id"]
            == json!(owner.session_id())
            && status_before["structuredContent"]["owner"]["capture_id"] == capture_id,
        "status must report the owning session and capture_id: {status_before}"
    );

    let error = intruder
        .tools_call_error("livecapture", json!({"operation": "stop"}))
        .await?;
    let message = error["message"].as_str().unwrap_or_default();
    ensure!(
        error["data"]["code"] == json!(synapse_core::error_codes::CAPTURE_OWNED_BY_OTHER_SESSION)
            && message.contains(owner.session_id()),
        "#112: non-owner stop must fail with CAPTURE_OWNED_BY_OTHER_SESSION naming owner: {error}"
    );

    let status_after = reader
        .tools_call("livecapture", json!({"operation": "status"}))
        .await?;
    ensure!(
        status_after["structuredContent"]["runtime"]["status"] == json!("running")
            && status_after["structuredContent"]["owner"]["owner_session_id"]
                == json!(owner.session_id()),
        "refused non-owner stop must leave capture running under original owner: {status_after}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_owner_stop_releases_capture_for_next_session_to_start() -> anyhow::Result<()> {
    let win_a = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let win_b = PaintedWindow::open(WindowSpec::visible(320, 240, BLUE));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut first_owner = daemon.session("g6c-112-first", &[]).await?;
    let mut second_owner = daemon.session("g6c-112-second", &[]).await?;

    first_owner
        .tools_call("livecapture", start_args(win_a.hwnd()))
        .await?;
    let stopped = first_owner
        .tools_call("livecapture", json!({"operation": "stop"}))
        .await?;
    ensure!(
        stopped["structuredContent"]["stopped"] == json!(true),
        "owner stop must succeed: {stopped}"
    );

    second_owner
        .tools_call("livecapture", start_args(win_b.hwnd()))
        .await?;
    let status = second_owner
        .tools_call("livecapture", json!({"operation": "status"}))
        .await?;
    ensure!(
        status["structuredContent"]["runtime"]["status"] == json!("running")
            && status["structuredContent"]["owner"]["owner_session_id"]
                == json!(second_owner.session_id()),
        "after owner stop, second session must own the new capture: {status}"
    );
    drop(daemon.stop().await?);
    Ok(())
}
