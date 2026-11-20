//! Regression for rejected MCP session headers creating phantom registry rows (#35).

use std::{net::TcpListener, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::process::{Child, Command};

const TOKEN: &str = "syn35-unknown-session-test-token";
const PROTOCOL: &str = "2025-11-25";

#[tokio::test]
async fn must_fail_rejected_session_headers_do_not_create_registry_rows() -> anyhow::Result<()> {
    let directory = tempfile::Builder::new()
        .prefix("synapse-unknown-sessions")
        .tempdir()?;
    let (mut daemon, endpoint) = start_daemon(directory.path()).await?;
    let result: anyhow::Result<()> = async {
        let client = reqwest::Client::new();
        let session_id = initialize(&client, &endpoint, "syn35-regression").await?;
        let baseline = registry_count(&client, &endpoint, &session_id).await?;
        ensure!(
            baseline == 1,
            "expected one initialized row, got {baseline}"
        );

        for index in 0..3 {
            let forged = format!("syn35-forged-{index}");
            let response = client
                .post(&endpoint)
                .bearer_auth(TOKEN)
                .header(ACCEPT, "application/json, text/event-stream")
                .header("Mcp-Session-Id", &forged)
                .header("MCP-Protocol-Version", PROTOCOL)
                .json(&json!({"jsonrpc": "2.0", "id": index + 10,
                              "method": "tools/list", "params": {}}))
                .send()
                .await?;
            ensure!(
                response.status() == reqwest::StatusCode::NOT_FOUND,
                "forged {forged} returned {}",
                response.status()
            );
        }
        let after = registry_count(&client, &endpoint, &session_id).await?;
        ensure!(
            after == baseline,
            "rejected headers grew the registry from {baseline} to {after}"
        );
        Ok(())
    }
    .await;
    stop_daemon(&mut daemon).await;
    result
}

#[tokio::test]
async fn must_pass_closed_peer_stays_until_its_ttl_then_is_swept() -> anyhow::Result<()> {
    let directory = tempfile::Builder::new()
        .prefix("synapse-closed-sessions")
        .tempdir()?;
    let (mut daemon, endpoint) = start_daemon(directory.path()).await?;
    let result: anyhow::Result<()> = async {
        let client = reqwest::Client::new();
        let session_id = initialize(&client, &endpoint, "syn35-regression").await?;
        let peer_id = initialize(&client, &endpoint, "syn35-retired-peer").await?;
        ensure!(
            registry_count(&client, &endpoint, &session_id).await? == 2,
            "initialized peer did not enter the registry"
        );
        let deleted = client
            .delete(&endpoint)
            .bearer_auth(TOKEN)
            .header("Mcp-Session-Id", &peer_id)
            .header("MCP-Protocol-Version", PROTOCOL)
            .send()
            .await?;
        ensure!(deleted.status().is_success(), "peer DELETE failed");
        ensure!(
            registry_count(&client, &endpoint, &session_id).await? == 2,
            "closed peer disappeared before its TTL"
        );

        let deadline = tokio::time::Instant::now() + Duration::from_secs(36);
        loop {
            let count = registry_count(&client, &endpoint, &session_id).await?;
            if count == 1 {
                break;
            }
            ensure!(
                count == 2,
                "unexpected registry count during retention: {count}"
            );
            ensure!(
                tokio::time::Instant::now() < deadline,
                "closed peer remained after TTL and retention sweep"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(())
    }
    .await;
    stop_daemon(&mut daemon).await;
    result
}

/// Starts an isolated HTTP daemon with a 2 s idle TTL; returns it and its MCP endpoint.
async fn start_daemon(directory: &std::path::Path) -> anyhow::Result<(Child, String)> {
    let bind = TcpListener::bind("127.0.0.1:0")?.local_addr()?.to_string();
    let daemon = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .args([
            "--mode",
            "http",
            "--bind",
            &bind,
            "--db",
            &directory.join("db").to_string_lossy(),
        ])
        .env("APPDATA", directory)
        .env("LOCALAPPDATA", directory)
        .env("SYNAPSE_LOG_DIR", directory)
        .env("SYNAPSE_BEARER_TOKEN", TOKEN)
        .env("SYNAPSE_HTTP_SESSION_IDLE_TIMEOUT_SECS", "2")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated daemon")?;
    let client = reqwest::Client::new();
    let mut healthy = false;
    for _ in 0..100 {
        if client
            .get(format!("http://{bind}/health"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            healthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A daemon that never became healthy is a setup failure, never a silent
    // fall-through into assertions against a dead endpoint.
    ensure!(
        healthy,
        "isolated daemon at {bind} never answered /health within 10 s"
    );
    Ok((daemon, format!("http://{bind}/mcp")))
}

async fn stop_daemon(daemon: &mut Child) {
    let _ = daemon.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), daemon.wait()).await;
}

async fn initialize(
    client: &reqwest::Client,
    endpoint: &str,
    client_name: &str,
) -> anyhow::Result<String> {
    let initialized = client
        .post(endpoint)
        .bearer_auth(TOKEN)
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": PROTOCOL, "capabilities": {},
                       "clientInfo": {"name": client_name, "version": "1"}}
        }))
        .send()
        .await?;
    ensure!(
        initialized.status().is_success(),
        "{client_name} initialize failed"
    );
    Ok(initialized
        .headers()
        .get("mcp-session-id")
        .context("initialize missing session id")?
        .to_str()?
        .to_owned())
}

async fn registry_count(
    client: &reqwest::Client,
    endpoint: &str,
    session_id: &str,
) -> anyhow::Result<u64> {
    let response = client
        .post(endpoint)
        .bearer_auth(TOKEN)
        .header(ACCEPT, "application/json, text/event-stream")
        .header("Mcp-Session-Id", session_id)
        .header("MCP-Protocol-Version", PROTOCOL)
        .json(&json!({
            "jsonrpc": "2.0", "id": 100, "method": "tools/call",
            "params": {"name": "session", "arguments": {"operation": "list"}}
        }))
        .send()
        .await?;
    ensure!(response.status().is_success(), "session list HTTP failure");
    let body = response.text().await?;
    let frame = body
        .lines()
        .find_map(|line| line.strip_prefix("data: {").map(|rest| format!("{{{rest}")))
        .context("session list missing JSON-RPC frame")?;
    let rpc: Value = serde_json::from_str(&frame)?;
    rpc.pointer("/result/structuredContent/list/registry_entry_count")
        .and_then(Value::as_u64)
        .with_context(|| format!("session list missing registry count: {rpc}"))
}
