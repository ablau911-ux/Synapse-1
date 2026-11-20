//! #172 through the REAL daemon and a schema-validating client: when the
//! diagnostic tool ledger (`<db>/daemon-tool-last.json`,
//! `<db>/daemon-tool-events.jsonl`) cannot be written, a side-effecting tool
//! call must still return its real success result (no JSON-RPC error, no
//! isError) and must have executed exactly once, so a client never retries
//! and repeats the side effect. Source of truth: the workspace CF_KV row
//! (`workspace exists`: exists + current_version).

#[allow(dead_code)]
mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;

const RUN_ID: &str = "g1b-172";

async fn put(session: &mut HttpMcpSession, key: &str) -> anyhow::Result<Value> {
    let result = session
        .tools_call(
            "workspace",
            json!({"operation": "put", "put": {"run_id": RUN_ID, "key": key, "value": {"n": 1}}}),
        )
        .await?;
    Ok(result["structuredContent"]["put"].clone())
}

async fn exists(session: &mut HttpMcpSession, key: &str) -> anyhow::Result<Value> {
    let result = session
        .tools_call(
            "workspace",
            json!({"operation": "exists", "exists": {"run_id": RUN_ID, "key": key}}),
        )
        .await?;
    Ok(result["structuredContent"]["exists"].clone())
}

/// Replaces a ledger file with a directory so every write to it fails.
fn block(path: &Path) -> anyhow::Result<()> {
    if path.is_file() {
        std::fs::remove_file(path)?;
    }
    std::fs::create_dir_all(path)?;
    assert!(path.is_dir(), "precondition: {} is blocked", path.display());
    Ok(())
}

#[tokio::test]
async fn must_pass_healthy_ledger_records_the_side_effecting_call() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g1b-172-healthy", &[]).await?;
    let written = put(&mut session, "healthy").await?;
    assert_eq!(written["version"], 1, "{written}");
    let last: Value = serde_json::from_str(&std::fs::read_to_string(
        daemon.db_path().join("daemon-tool-last.json"),
    )?)?;
    assert_eq!(last["tool"], "workspace", "ledger readback: {last}");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_blocked_ledger_never_turns_an_executed_write_into_an_error() -> anyhow::Result<()>
{
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g1b-172-blocked", &[]).await?;
    session.tools_call("health", json!({})).await?;
    let db = daemon.db_path().to_path_buf();
    assert!(
        db.join("daemon-tool-last.json").is_file(),
        "precondition: real ledger path"
    );
    block(&db.join("daemon-tool-last.json"))?;
    block(&db.join("daemon-tool-events.jsonl"))?;

    // tools_call fails on a JSON-RPC error, isError=true or an invalid outputSchema.
    let written = put(&mut session, "blocked-ledger").await?;
    assert_eq!(written["version"], 1, "{written}");

    let row = exists(&mut session, "blocked-ledger").await?;
    assert_eq!(row["exists"], true, "{row}");
    assert_eq!(
        row["current_version"], 1,
        "the write ran exactly once: {row}"
    );
    assert!(
        db.join("daemon-tool-last.json").is_dir(),
        "ledger stayed blocked during the call"
    );
    drop(daemon.stop().await?);
    Ok(())
}

/// Variant (same class as #172): an unreadable diagnostic file left by a
/// previous run must not stop the daemon from serving tool calls.
#[tokio::test]
async fn must_fail_corrupt_previous_tool_ledger_does_not_block_startup() -> anyhow::Result<()> {
    let home = tempfile::Builder::new()
        .prefix("synapse-g1b-172-")
        .tempdir()?;
    let db = home.path().join("db");
    std::fs::create_dir_all(&db)?;
    std::fs::write(db.join("daemon-tool-last.json"), "{ not json")?;
    let db_arg = db.to_string_lossy().into_owned();

    let mut client =
        StdioMcpClient::launch_and_init_with_env(None, &[("SYNAPSE_DB", db_arg.as_str())]).await?;
    client.tools_call("health", json!({})).await?;
    assert!(client.shutdown().await?.success());
    Ok(())
}
