//! Regression tests for #110: repeated `livecapture` `start` on the same
//! window and config reuses the running capture without advancing `generation`
//! or invalidating prior `next_cursor`s, returns `frame_reused: true` when
//! no newer frame is queued, and only invalidates cursors after an actual
//! stop/retarget.

#![cfg(windows)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use std::time::Duration;

use anyhow::{Context, ensure};
use g6a_window::{BLUE, GREEN, PaintedWindow, RED, WindowSpec};
use serde_json::{Value, json};
use support::HttpDaemon;

const PAINT_TIMEOUT: Duration = Duration::from_secs(5);

fn start_args(hwnd: i64) -> Value {
    json!({"operation": "start", "window_hwnd": hwnd})
}

fn next_args(hwnd: i64, cursor: &Value) -> Value {
    json!({
        "operation": "next",
        "window_hwnd": hwnd,
        "after_frame": cursor,
        "frame_count": 1,
        "wait_timeout_ms": 3000
    })
}

#[tokio::test]
async fn must_pass_repeated_start_preserves_cursor_for_next_and_reuses_static_frame()
-> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let hwnd = window.hwnd();
    let daemon = HttpDaemon::start(&[]).await?;
    let mut owner = daemon.session("g6c-110-owner", &[]).await?;
    let mut adopter = daemon.session("g6c-110-adopter", &[]).await?;

    let first = owner.tools_call("livecapture", start_args(hwnd)).await?;
    let cursor_1 = first["structuredContent"]["next_cursor"].clone();
    ensure!(
        first["structuredContent"]["frames"][0]["frame"]["frame_reused"] == json!(false),
        "initial start must deliver a fresh frame: {first}"
    );
    let second = adopter.tools_call("livecapture", start_args(hwnd)).await?;
    let cursor_2 = second["structuredContent"]["next_cursor"].clone();
    ensure!(
        cursor_2["capture_id"] == cursor_1["capture_id"]
            && cursor_2["generation"] == cursor_1["generation"],
        "#110: repeated start must preserve capture_id and generation ({cursor_1} vs {cursor_2})"
    );
    let third = adopter.tools_call("livecapture", start_args(hwnd)).await?;
    ensure!(
        third["structuredContent"]["frames"][0]["frame"]["frame_reused"] == json!(true),
        "#110: repeated start on static window must reuse cached frame: {third}"
    );

    window.set_color(GREEN);
    ensure!(
        window.wait_painted(GREEN, PAINT_TIMEOUT),
        "window repainted GREEN"
    );
    let next = owner
        .tools_call("livecapture", next_args(hwnd, &cursor_1))
        .await?;
    let next_frame = &next["structuredContent"]["frames"][0]["frame"];
    let seq_1 = cursor_1["frame_seq"]
        .as_u64()
        .context("cursor_1 frame_seq")?;
    let seq_next = next_frame["frame_seq"].as_u64().context("next frame_seq")?;
    ensure!(
        next_frame["capture_id"] == cursor_1["capture_id"]
            && next_frame["generation"] == cursor_1["generation"]
            && seq_next > seq_1
            && next_frame["frame_reused"] == json!(false),
        "#110: cursor_1 from first start must remain valid after repeated starts: {next}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_stop_and_restart_advances_generation_and_rejects_old_cursor()
-> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let hwnd = window.hwnd();
    let daemon = HttpDaemon::start(&[]).await?;
    let mut owner = daemon.session("g6c-110-restart", &[]).await?;

    let first = owner.tools_call("livecapture", start_args(hwnd)).await?;
    let cursor_1 = first["structuredContent"]["next_cursor"].clone();
    let gen_1 = cursor_1["generation"].as_u64().context("gen_1")?;
    owner
        .tools_call("livecapture", json!({"operation": "stop"}))
        .await?;

    let restarted = owner.tools_call("livecapture", start_args(hwnd)).await?;
    let cursor_2 = restarted["structuredContent"]["next_cursor"].clone();
    let gen_2 = cursor_2["generation"].as_u64().context("gen_2")?;
    ensure!(
        gen_2 > gen_1 && cursor_2["capture_id"] != cursor_1["capture_id"],
        "stop + start must advance generation and rotate capture_id: {cursor_1} -> {cursor_2}"
    );
    let error = owner
        .tools_call_error("livecapture", next_args(hwnd, &cursor_1))
        .await?;
    ensure!(
        error["data"]["code"] == json!(synapse_core::error_codes::CAPTURE_FRAME_UNAVAILABLE),
        "stale cursor after stop+start must fail with CAPTURE_FRAME_UNAVAILABLE (cursor_stale): {error}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_non_owner_retarget_start_is_refused_and_keeps_owner_cursor_valid()
-> anyhow::Result<()> {
    let win_a = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let win_b = PaintedWindow::open(WindowSpec::visible(320, 240, BLUE));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut owner = daemon.session("g6c-110-win-a-owner", &[]).await?;
    let mut other = daemon.session("g6c-110-win-b-other", &[]).await?;

    let started = owner
        .tools_call("livecapture", start_args(win_a.hwnd()))
        .await?;
    let cursor_a = started["structuredContent"]["next_cursor"].clone();
    let refusal = other
        .tools_call_error("livecapture", start_args(win_b.hwnd()))
        .await?;
    ensure!(
        refusal["data"]["code"] == json!(synapse_core::error_codes::CAPTURE_OWNED_BY_OTHER_SESSION),
        "retargeting start by non-owner must be refused: {refusal}"
    );

    win_a.set_color(GREEN);
    ensure!(
        win_a.wait_painted(GREEN, PAINT_TIMEOUT),
        "win_a repainted GREEN"
    );
    let next = owner
        .tools_call("livecapture", next_args(win_a.hwnd(), &cursor_a))
        .await?;
    ensure!(
        next["structuredContent"]["frames"][0]["frame"]["capture_id"] == cursor_a["capture_id"],
        "owner cursor must still work after refused non-owner retarget: {next}"
    );
    drop(daemon.stop().await?);
    Ok(())
}
