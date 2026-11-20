//! Shared readbacks for the G6a screenshot/GIF/livecapture integration tests.
//!
//! - `daemon-tool-events.jsonl` (the daemon's own tool ledger under `--db`) is
//!   the physical source of truth for "this tool call has started" and for its
//!   final status, so tests poll it instead of sleeping for ordering.
//! - GIF artifacts are decoded from disk, never trusted from the response.
//! - `RawRpc` sends a JSON-RPC request with a caller-chosen id (so the test can
//!   cancel exactly that request) after validating the arguments against the
//!   live `tools/list` schema like the production client (D1 client parity).

#![allow(dead_code)]

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use image::{AnimationDecoder, RgbaImage, codecs::gif::GifDecoder};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use synapse_test_utils::tool_catalog::ToolCatalog;

/// Same bearer the shared `support::HttpDaemon` configures (test-only secret).
const BEARER: &str = "synapse-tests-http-bearer";
const PROTOCOL_VERSION: &str = "2025-06-18";
const TOOL_EVENTS_FILE: &str = "daemon-tool-events.jsonl";

/// Every row of the daemon tool ledger (unparsable lines are skipped).
pub fn tool_events(db_path: &Path) -> Vec<Value> {
    std::fs::read_to_string(db_path.join(TOOL_EVENTS_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

/// Ledger rows for `tool` with `status`.
pub fn tool_rows(db_path: &Path, tool: &str, status: &str) -> Vec<Value> {
    tool_events(db_path)
        .into_iter()
        .filter(|row| row["tool"] == json!(tool) && row["status"] == json!(status))
        .collect()
}

/// Polls the ledger until `count` rows of `tool` have `status=started`.
pub async fn wait_tool_started(
    db_path: &Path,
    tool: &str,
    count: usize,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if tool_rows(db_path, tool, "started").len() >= count {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The ledger starts before WGC acquisition. Wait for a verified GIF frame
/// in the isolated daemon's physical JSON logs before destroying its target.
pub async fn wait_gif_frame_captured(log_dir: &Path, hwnd: i64, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let logs = std::fs::read_dir(log_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("synapse.log.")
            });
        for entry in logs {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|row| {
                    row["fields"]["code"] == "CAPTURE_GIF_FIRST_FRAME_CAPTURED"
                        && row["fields"]["window_hwnd"].as_i64() == Some(hwnd)
                })
            {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Polls the ledger until session `session_id` has a `started` row for `tool`.
pub async fn wait_session_tool_started(
    db_path: &Path,
    tool: &str,
    session_id: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if tool_rows(db_path, tool, "started")
            .iter()
            .any(|row| row["mcp_session_id"] == json!(session_id))
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `error.data.code` of a JSON-RPC error object.
pub fn error_code(error: &Value) -> String {
    error["data"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// Decodes every frame of the GIF at `path` from disk.
pub fn gif_frames(path: &Path) -> anyhow::Result<Vec<RgbaImage>> {
    let bytes = std::fs::read(path).with_context(|| format!("read GIF {}", path.display()))?;
    let decoder = GifDecoder::new(std::io::Cursor::new(bytes)).context("GIF header")?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .context("decode GIF frames")?;
    Ok(frames.into_iter().map(image::Frame::into_buffer).collect())
}

/// RGB of the centre pixel of `frame`.
pub fn centre_rgb(frame: &RgbaImage) -> [u8; 3] {
    let pixel = frame.get_pixel(frame.width() / 2, frame.height() / 2);
    [pixel[0], pixel[1], pixel[2]]
}

/// One MCP session driven with raw JSON-RPC ids (for cancellation tests).
pub struct RawRpc {
    client: reqwest::Client,
    endpoint: String,
    session_id: String,
    catalog: ToolCatalog,
}

impl RawRpc {
    /// Wraps an initialized session id; `tools_list` is the body returned by
    /// that session's validated `tools/list`.
    pub fn new(endpoint: String, session_id: &str, tools_list: &Value) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::new(),
            endpoint,
            session_id: session_id.to_owned(),
            catalog: ToolCatalog::from_tools_list(tools_list)?,
        })
    }

    /// Schema-validated `tools/call` with JSON-RPC id `id`; returns the whole
    /// JSON-RPC response frame.
    pub async fn tools_call(&self, id: u64, name: &str, args: Value) -> anyhow::Result<Value> {
        self.catalog.check_arguments(name, &args)?;
        let body = json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": args}
        });
        let response = self.post(&body).await?;
        crate::support::decode_mcp_response(response).await
    }

    /// `notifications/cancelled` for request `id`.
    pub async fn cancel(&self, id: u64, reason: &str) -> anyhow::Result<()> {
        let body = json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": {"requestId": id, "reason": reason}
        });
        let response = self.post(&body).await?;
        ensure!(
            response.status().is_success(),
            "cancel HTTP {}",
            response.status()
        );
        Ok(())
    }

    async fn post(&self, body: &Value) -> anyhow::Result<reqwest::Response> {
        self.client
            .post(&self.endpoint)
            .bearer_auth(BEARER)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header("Mcp-Session-Id", &self.session_id)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(body)
            .send()
            .await
            .context("send raw MCP request")
    }
}
