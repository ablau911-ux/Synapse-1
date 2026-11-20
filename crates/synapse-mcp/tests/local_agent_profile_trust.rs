use std::{net::TcpListener, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use serde_json::{Value, json};
use tokio::process::Command;

const BEARER: &str = "syn55-isolated-bearer";
const CAPABILITY: &str = "syn55-isolated-local-agent-capability-0123456789";

/// `(client name, capability header, expected effective profile)`.
type ProfileRow<'a> = (&'a str, Option<&'a str>, &'a str);

#[tokio::test]
async fn must_fail_client_name_cannot_grant_local_agent_profile() -> anyhow::Result<()> {
    assert_session_profiles(&[
        ("synapse-local-model-agent", None, "normal_agent"),
        (
            "local-model-spoof",
            Some("wrong-capability"),
            "normal_agent",
        ),
        ("ordinary-client", Some(CAPABILITY), "normal_agent"),
    ])
    .await
}

#[tokio::test]
async fn must_pass_local_agent_name_with_the_capability_gets_full_capability() -> anyhow::Result<()>
{
    assert_session_profiles(&[(
        "synapse-local-model-agent",
        Some(CAPABILITY),
        "full_capability",
    )])
    .await
}

/// Starts an isolated HTTP daemon, initializes one MCP session per row and
/// checks the effective profile `profile operation=status` reports for it.
async fn assert_session_profiles(rows: &[ProfileRow<'_>]) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind = listener.local_addr()?.to_string();
    drop(listener);
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .args(["--mode", "http", "--bind", &bind, "--db"])
        .arg(directory.path().join("db"))
        .env("APPDATA", directory.path())
        .env("LOCALAPPDATA", directory.path())
        .env("SYNAPSE_LOG_DIR", directory.path())
        .env("SYNAPSE_BEARER_TOKEN", BEARER)
        .env("SYNAPSE_LOCAL_AGENT_CAPABILITY", CAPABILITY)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated daemon")?;

    let result = async {
        let client = reqwest::Client::new();
        let url = format!("http://{bind}/mcp");
        let health = format!("http://{bind}/health");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(response) = client.get(&health).bearer_auth(BEARER).send().await
                && response.status().is_success()
            {
                break;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "daemon health timeout"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        for &(name, proof, expected) in rows {
            let mut initialize = client
                .post(&url)
                .bearer_auth(BEARER)
                .header("Accept", "application/json, text/event-stream")
                .json(
                    &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                    "protocolVersion":"2025-03-26","capabilities":{},
                    "clientInfo":{"name":name,"version":"1.0"}}}),
                );
            if let Some(proof) = proof {
                initialize = initialize.header("X-Synapse-Local-Agent-Capability", proof);
            }
            let response = initialize.send().await?;
            ensure!(
                response.status().is_success(),
                "initialize {name}: {}",
                response.status()
            );
            let session_id = response
                .headers()
                .get("Mcp-Session-Id")
                .context("missing MCP session header")?
                .to_str()?
                .to_owned();
            let response = client
                .post(&url)
                .bearer_auth(BEARER)
                .header("Accept", "application/json, text/event-stream")
                .header("Mcp-Session-Id", &session_id)
                .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
                    "params":{"name":"profile","arguments":{"operation":"status"}}}))
                .send()
                .await?;
            ensure!(
                response.status().is_success(),
                "profile call {name}: {}",
                response.status()
            );
            let body = response.text().await?;
            let data = body
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .find(|line| !line.trim().is_empty())
                .unwrap_or(&body);
            let payload: Value =
                serde_json::from_str(data.trim()).context("decode MCP profile response")?;
            ensure!(payload["error"].is_null(), "profile error: {payload}");
            let tool_json = payload["result"]["content"][0]["text"]
                .as_str()
                .context("missing profile tool text")?;
            let tool: Value = serde_json::from_str(tool_json)?;
            let profile = tool["status"]["snapshot"]["profile"].as_str();
            ensure!(
                profile == Some(expected),
                "{name}: expected {expected}, got {profile:?}"
            );
        }
        Ok(())
    }
    .await;

    let _ = reqwest::Client::new()
        .post(format!("http://{bind}/shutdown"))
        .bearer_auth(BEARER)
        .send()
        .await;
    if tokio::time::timeout(Duration::from_secs(10), daemon.wait())
        .await
        .is_err()
    {
        let _ = daemon.start_kill();
        let _ = daemon.wait().await;
    }
    result
}
