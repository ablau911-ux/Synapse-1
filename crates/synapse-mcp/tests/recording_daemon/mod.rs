//! Shared harness for G5a act_* integration tests: the REAL daemon over the
//! REAL stdio MCP transport (schema-validating client), with the recording
//! action backend so no input ever reaches the owner's desktop, and a
//! synthetic Notepad foreground so the supported-use gate is deterministic.
//!
//! Sources of truth read back by the tests:
//! - daemon log lines `readback=recording_backend tool=<tool>` (one per
//!   action the recording backend executed);
//! - CF_ACTION_LOG rows, read from the RocksDB after the daemon exited.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};
use synapse_core::SCHEMA_VERSION;
use synapse_storage::{Db, cf, decode_json};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

/// One isolated daemon: its own DB and log directory.
pub struct RecordingDaemon {
    pub client: StdioMcpClient,
    pub logs: TempDir,
    pub db_dir: TempDir,
}

impl RecordingDaemon {
    pub async fn launch() -> anyhow::Result<Self> {
        let logs = TempDir::new()?;
        let db_dir = TempDir::new()?;
        let db_path = db_dir.path().join("db").to_string_lossy().into_owned();
        let mut client = StdioMcpClient::launch_and_init_with_env(
            Some(logs.path()),
            &[
                ("SYNAPSE_DB", db_path.as_str()),
                ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
                ("SYNAPSE_MCP_RECORDING_BACKEND", "1"),
                ("SYNAPSE_DEBUG_TOOLS", "1"),
            ],
        )
        .await?;
        client
            .tools_call("profile_activate", json!({"profile_id": "notepad"}))
            .await
            .context("precondition: notepad profile activates")?;
        Ok(Self {
            client,
            logs,
            db_dir,
        })
    }

    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.db_dir.path().join("db")
    }

    /// Stops the daemon (releases the RocksDB lock) and returns the log text
    /// plus every CF_ACTION_LOG row in key order.
    pub async fn finish(self) -> anyhow::Result<(String, Vec<Value>)> {
        let db_path = self.db_path();
        let status = self.client.shutdown().await?;
        anyhow::ensure!(status.success(), "daemon exit status {status:?}");
        let logs = read_logs(self.logs.path())?;
        let rows = action_log_rows(&db_path)?;
        Ok((logs, rows))
    }
}

/// Number of actions the recording backend executed for `tool`.
#[must_use]
pub fn recording_count(logs: &str, tool: &str) -> usize {
    logs.matches(&format!("readback=recording_backend tool={tool} "))
        .count()
}

pub fn read_logs(path: &Path) -> anyhow::Result<String> {
    let mut logs = String::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            logs.push_str(&std::fs::read_to_string(entry.path())?);
        }
    }
    Ok(logs)
}

pub fn action_log_rows(db_path: &Path) -> anyhow::Result<Vec<Value>> {
    let db = Db::open(db_path, SCHEMA_VERSION)?;
    let mut rows = db.scan_cf(cf::CF_ACTION_LOG)?;
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows.into_iter()
        .map(|(_key, value)| decode_json::<Value>(&value).map_err(Into::into))
        .collect()
}

/// Rows of `tool` with `status`, in write order.
#[must_use]
pub fn rows_of<'a>(rows: &'a [Value], tool: &str, status: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|row| row["tool"] == tool && row["status"] == status)
        .collect()
}

/// `data` object of a JSON-RPC error returned by `tools_call_error`.
#[must_use]
pub fn error_data(error: &Value) -> &Value {
    &error["data"]
}

/// `structuredContent` of a successful `tools_call`.
pub fn structured(response: &Value) -> anyhow::Result<Value> {
    response
        .get("structuredContent")
        .cloned()
        .context("structuredContent missing")
}
