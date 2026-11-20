//! #48: the GIF route (`screenshot operation=gif`, `capture_gif`) must not run
//! its blocking WGC capture / resize / GIF encode / file write on a tokio
//! worker. Screenshot stills were moved to `spawn_blocking`; the GIF sibling
//! (`server/capture_gif.rs` capture loop) was not.
//!
//! Discriminator: the daemon runs with ONE tokio worker (`TOKIO_WORKER_THREADS=1`,
//! honoured by `tokio::runtime::Builder::new_multi_thread`). A GIF of a
//! test-owned MINIMIZED window must be refused promptly (#224), without a WGC
//! timeout or output file. A concurrent second session's `tools/list` must stay
//! responsive. The visible-window case exercises responsiveness during capture.
//!
//! Only synthetic test-owned windows are captured; artifacts live in the
//! daemon's temp home and are removed with it.

#![cfg(windows)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use std::time::{Duration, Instant};

use anyhow::ensure;
use g6a_support::{centre_rgb, error_code, gif_frames, wait_tool_started};
use g6a_window::{PaintedWindow, RED, WindowSpec, classify_rgb};
use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};

/// A healthy daemon answers a cached `tools/list` far below this.
const PROBE_LIMIT: Duration = Duration::from_millis(1500);
const ONE_WORKER: &[(&str, &str)] = &[("TOKIO_WORKER_THREADS", "1")];

fn gif_args(path: &std::path::Path, hwnd: i64, duration_ms: u64) -> Value {
    json!({
        "operation": "gif", "path": path.to_string_lossy(), "window_hwnd": hwnd,
        "duration_ms": duration_ms, "interval_ms": 100
    })
}

/// Latency of one `tools/list` on `probe`, plus the instant it completed.
async fn probe(session: &mut HttpMcpSession) -> anyhow::Result<(Duration, Instant)> {
    let started = Instant::now();
    session.tools_list().await?;
    Ok((started.elapsed(), Instant::now()))
}

/// Opens the GIF session and the probe session; probes once as a baseline.
async fn sessions(daemon: &HttpDaemon) -> anyhow::Result<(HttpMcpSession, HttpMcpSession)> {
    let mut gif = daemon.session("g6a-48-gif", &[]).await?;
    gif.tools_list().await?;
    let mut probe_session = daemon.session("g6a-48-probe", &[]).await?;
    let (baseline, _) = probe(&mut probe_session).await?;
    ensure!(
        baseline < PROBE_LIMIT,
        "precondition: idle tools/list must be fast, took {baseline:?}"
    );
    Ok((gif, probe_session))
}

#[tokio::test]
async fn must_fail_minimized_target_gif_does_not_stall_other_sessions() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::minimized(RED));
    ensure!(window.is_minimized(), "precondition: target is minimized");
    let daemon = HttpDaemon::start(ONE_WORKER).await?;
    let (mut gif, mut probe_session) = sessions(&daemon).await?;
    let path = daemon.home().join("minimized-48.gif");
    ensure!(!path.exists(), "precondition: {} absent", path.display());

    let args = gif_args(&path, window.hwnd(), 200);
    let gif_call = async {
        let started = Instant::now();
        let error = gif.tools_call_error("screenshot", args).await;
        (error, started.elapsed())
    };
    let ((error, gif_elapsed), probe_result) = tokio::join!(gif_call, probe(&mut probe_session));
    let (latency, _) = probe_result?;
    let error = error?;

    ensure!(
        error_code(&error) == synapse_core::error_codes::ACTION_NO_OBSERVED_DELTA,
        "minimized-target GIF must be ACTION_NO_OBSERVED_DELTA: {error}"
    );
    ensure!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("CAPTURE_TARGET_MINIMIZED"),
        "minimized-target GIF must report CAPTURE_TARGET_MINIMIZED (#224): {error}"
    );
    ensure!(
        !path.exists(),
        "a failed GIF must not publish {}",
        path.display()
    );
    ensure!(
        gif_elapsed < PROBE_LIMIT,
        "#224: minimized-target GIF must fail promptly without a WGC timeout, took {gif_elapsed:?}"
    );
    ensure!(
        latency < PROBE_LIMIT,
        "#48: concurrent tools/list on another session took {latency:?} \
         during minimized-target GIF refusal (GIF took {gif_elapsed:?})"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_visible_gif_records_while_other_sessions_stay_responsive() -> anyhow::Result<()>
{
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let daemon = HttpDaemon::start(ONE_WORKER).await?;
    let (mut gif, mut probe_session) = sessions(&daemon).await?;
    let path = daemon.home().join("visible-48.gif");

    let args = gif_args(&path, window.hwnd(), 1500);
    let task = tokio::spawn(async move { gif.tools_call("screenshot", args).await });
    ensure!(
        wait_tool_started(daemon.db_path(), "screenshot", 1, Duration::from_secs(10)).await,
        "GIF call never reached the daemon tool ledger"
    );
    let mut worst = Duration::ZERO;
    while !task.is_finished() {
        let (latency, _) = probe(&mut probe_session).await?;
        worst = worst.max(latency);
    }
    let response = task.await??;

    let gif_result = &response["structuredContent"]["gif"];
    let frames_captured = gif_result["frames_captured"].as_u64().unwrap_or_default();
    ensure!(frames_captured >= 1, "GIF must capture frames: {response}");
    let frames = gif_frames(&path)?;
    ensure!(
        frames.len() as u64 == frames_captured,
        "decoded frame count {} != reported {frames_captured}",
        frames.len()
    );
    ensure!(
        frames
            .iter()
            .all(|frame| classify_rgb(centre_rgb(frame)) == Some(RED)),
        "every frame must show the red test window"
    );
    ensure!(
        gif_result["bytes_written"].as_u64() == Some(std::fs::metadata(&path)?.len()),
        "bytes_written must equal the file on disk: {response}"
    );
    ensure!(
        worst < PROBE_LIMIT,
        "#48: a probe waited {worst:?} during the GIF recording"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_existing_gif_without_overwrite_is_refused_and_unchanged() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(200, 150, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-48-overwrite", &[]).await?;
    let path = daemon.home().join("existing-48.gif");
    std::fs::write(&path, b"GIF89a-sentinel")?;
    let before = std::fs::read(&path)?;

    let error = session
        .tools_call_error("screenshot", gif_args(&path, window.hwnd(), 300))
        .await?;
    ensure!(
        error_code(&error) == synapse_core::error_codes::TOOL_PARAMS_INVALID,
        "existing path without overwrite must be TOOL_PARAMS_INVALID: {error}"
    );
    ensure!(
        std::fs::read(&path)? == before,
        "the existing file must be unchanged"
    );
    drop(daemon.stop().await?);
    Ok(())
}
