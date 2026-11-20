//! #103: a cancelled public `screenshot operation=gif` request must stop and
//! must never publish its output file; cancellation is reported as RMCP
//! `-32800` / `REQUEST_CANCELLED` (contract: `server/capture_gif.rs`
//! `capture_gif_cancelled_error`, linearization comment before `persist`).
//!
//! The request is sent with a known JSON-RPC id, the daemon tool ledger's
//! `started` row proves it is in flight, then `notifications/cancelled` names
//! that id. Sources of truth read afterwards: the ledger's finish row and the
//! output path on disk, re-read after the whole recording window has passed.

#![cfg(windows)]

mod g6a_support;
mod g6a_window;
#[allow(dead_code)]
mod support;

use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use g6a_support::{RawRpc, tool_rows, wait_tool_started};
use g6a_window::{PaintedWindow, RED, WindowSpec, wait_until};
use serde_json::{Value, json};
use support::HttpDaemon;

const GIF_ID: u64 = 7_103;
const DURATION_MS: u64 = 4_000;

fn gif_args(path: &std::path::Path, hwnd: i64, overwrite: bool) -> Value {
    json!({
        "operation": "gif", "path": path.to_string_lossy(), "window_hwnd": hwnd,
        "duration_ms": DURATION_MS, "interval_ms": 200, "overwrite": overwrite
    })
}

/// The ledger's finished (non-`started`) screenshot row, polled with a deadline.
async fn finished_row(db: &std::path::Path) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(row) = tool_rows(db, "screenshot", "error")
            .into_iter()
            .chain(tool_rows(db, "screenshot", "ok"))
            .next()
        {
            return Ok(row);
        }
        ensure!(
            Instant::now() < deadline,
            "GIF call never finished in the ledger"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn must_fail_cancelled_gif_publishes_no_artifact() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-103", &[]).await?;
    let tools = session.tools_list().await?;
    let call_rpc = RawRpc::new(daemon.url("/mcp"), session.session_id(), &tools)?;
    let cancel_rpc = RawRpc::new(daemon.url("/mcp"), session.session_id(), &tools)?;
    let path = daemon.home().join("cancelled-103.gif");
    ensure!(!path.exists(), "precondition: {} absent", path.display());

    let args = gif_args(&path, window.hwnd(), false);
    let call = tokio::spawn(async move { call_rpc.tools_call(GIF_ID, "screenshot", args).await });
    ensure!(
        wait_tool_started(daemon.db_path(), "screenshot", 1, Duration::from_secs(10)).await,
        "GIF call never reached the daemon tool ledger"
    );
    let cancelled_at = Instant::now();
    cancel_rpc.cancel(GIF_ID, "g6a #103 regression").await?;

    let row = finished_row(daemon.db_path()).await?;
    ensure!(
        row["status"] == json!("error")
            && row["error"]["data"]["code"] == json!(synapse_core::error_codes::REQUEST_CANCELLED),
        "#103: cancelled GIF must finish as REQUEST_CANCELLED in the ledger: {row}"
    );
    if let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(10), call).await
        && let Ok(frame) = frame
        && let Some(error) = frame.get("error")
    {
        ensure!(
            error["code"] == json!(-32800),
            "cancel must be RMCP -32800: {error}"
        );
    }
    // Re-read the output path after the whole recording window has elapsed.
    let window_end = Duration::from_millis(DURATION_MS + 1_500);
    let published = wait_until(window_end.saturating_sub(cancelled_at.elapsed()), || {
        path.exists()
    });
    ensure!(
        !published,
        "#103: cancelled GIF published {}",
        path.display()
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_uncancelled_gif_publishes_the_artifact() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-103-ok", &[]).await?;
    let path = daemon.home().join("kept-103.gif");
    let mut args = gif_args(&path, window.hwnd(), false);
    args["duration_ms"] = json!(600);
    let response = session.tools_call("screenshot", args).await?;
    let bytes = std::fs::metadata(&path)
        .with_context(|| format!("uncancelled GIF missing at {}", path.display()))?
        .len();
    ensure!(
        response["structuredContent"]["gif"]["bytes_written"].as_u64() == Some(bytes),
        "bytes_written must match the published file: {response}"
    );
    let row = finished_row(daemon.db_path()).await?;
    ensure!(row["status"] == json!("ok"), "ledger must record ok: {row}");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_cancelled_gif_never_touches_a_pre_existing_file() -> anyhow::Result<()> {
    let window = PaintedWindow::open(WindowSpec::visible(320, 240, RED));
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g6a-103-existing", &[]).await?;
    let tools = session.tools_list().await?;
    let call_rpc = RawRpc::new(daemon.url("/mcp"), session.session_id(), &tools)?;
    let cancel_rpc = RawRpc::new(daemon.url("/mcp"), session.session_id(), &tools)?;
    let path = daemon.home().join("existing-103.gif");
    std::fs::write(&path, b"GIF89a-owner-file")?;
    let before = std::fs::read(&path)?;

    let args = gif_args(&path, window.hwnd(), true);
    let call = tokio::spawn(async move { call_rpc.tools_call(GIF_ID, "screenshot", args).await });
    ensure!(
        wait_tool_started(daemon.db_path(), "screenshot", 1, Duration::from_secs(10)).await,
        "GIF call never reached the daemon tool ledger"
    );
    cancel_rpc
        .cancel(GIF_ID, "g6a #103 overwrite regression")
        .await?;
    let row = finished_row(daemon.db_path()).await?;
    ensure!(
        row["error"]["data"]["code"] == json!(synapse_core::error_codes::REQUEST_CANCELLED),
        "cancelled overwrite must be REQUEST_CANCELLED: {row}"
    );
    let _ = tokio::time::timeout(Duration::from_secs(10), call).await;
    ensure!(
        std::fs::read(&path)? == before,
        "#103: cancel must leave the caller's file intact"
    );
    drop(daemon.stop().await?);
    Ok(())
}
