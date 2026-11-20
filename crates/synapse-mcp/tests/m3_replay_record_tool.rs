use anyhow::Context;
use serde_json::{Value, json};
use synapse_core::Observation;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

#[tokio::test]
async fn must_pass_replay_record_schema_defaults_and_file_output() -> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let local_app_data = TempDir::new()?;
    let replays = local_app_data.path().join("synapse").join("replays");
    let mut client = launch_replay_client(&logs, &local_app_data).await?;

    let tools = client.tools_list().await?;
    let tools = tools
        .get("tools")
        .and_then(Value::as_array)
        .context("tools array missing")?;
    let replay_record_tool = tools
        .iter()
        .find(|tool| tool["name"] == "replay_record")
        .context("replay_record tool missing")?;
    assert_replay_record_schema(replay_record_tool);

    let observation_path = replays.join("observations.jsonl");
    let response = client
        .tools_call(
            "replay_record",
            json!({"duration_ms": 25, "path": observation_path}),
        )
        .await?;
    let payload = structured(&response)?;
    assert_eq!(payload["observations_skipped"], 0);

    // Source of truth: the file on disk. The reported counts must equal it.
    let replay_text = std::fs::read_to_string(&observation_path)?;
    let observations = replay_text
        .lines()
        .map(serde_json::from_str::<Observation>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(!observations.is_empty());
    assert_eq!(
        payload["records_written"].as_u64(),
        Some(u64::try_from(observations.len())?),
        "records_written must equal the JSONL lines on disk: {payload}"
    );
    assert_eq!(
        payload["bytes"].as_u64(),
        Some(std::fs::metadata(&observation_path)?.len()),
        "bytes must equal the file length on disk: {payload}"
    );
    assert!(
        observations
            .iter()
            .all(|observation| observation.foreground.process_name == "notepad.exe")
    );

    let empty_path = replays.join("empty.jsonl");
    let empty = client
        .tools_call(
            "replay_record",
            json!({"duration_ms": 0, "path": empty_path}),
        )
        .await?;
    let empty_payload = structured(&empty)?;
    assert_eq!(empty_payload["records_written"], 0);
    assert_eq!(empty_payload["observations_skipped"], 0);
    assert_eq!(empty_payload["bytes"], 0);
    assert_eq!(std::fs::read_to_string(&empty_path)?, "");

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn must_fail_replay_record_unknown_target_and_format_are_refused() -> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let local_app_data = TempDir::new()?;
    let replays = local_app_data.path().join("synapse").join("replays");
    let mut client = launch_replay_client(&logs, &local_app_data).await?;

    let bad_target = client
        .tools_call_error(
            "replay_record",
            json!({"target": "bogus", "duration_ms": 1, "path": replays.join("bad-target.jsonl")}),
        )
        .await?;
    assert_eq!(bad_target["data"]["code"], "REPLAY_TARGET_INVALID");

    let bad_format = client
        .tools_call_error(
            "replay_record",
            json!({"format": "csv", "duration_ms": 1, "path": replays.join("bad-format.jsonl")}),
        )
        .await?;
    assert_eq!(bad_format["data"]["code"], "REPLAY_FORMAT_INVALID");
    for refused in ["bad-target.jsonl", "bad-format.jsonl"] {
        let path = replays.join(refused);
        assert!(
            !path.exists(),
            "refused replay_record created {}",
            path.display()
        );
    }

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

async fn launch_replay_client(
    logs: &TempDir,
    local_app_data: &TempDir,
) -> anyhow::Result<StdioMcpClient> {
    StdioMcpClient::launch_and_init_with_env(
        Some(logs.path()),
        &[
            ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
            (
                "LOCALAPPDATA",
                local_app_data
                    .path()
                    .to_str()
                    .context("LOCALAPPDATA path utf8")?,
            ),
        ],
    )
    .await
}

fn structured(response: &Value) -> anyhow::Result<Value> {
    if let Some(value) = response.get("structuredContent") {
        return Ok(value.clone());
    }

    let text = response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
        .context("structured content missing")?;
    serde_json::from_str(text).context("parse text content")
}

fn assert_replay_record_schema(tool: &Value) {
    let shape = json!({
        "name": tool.get("name").cloned().unwrap_or(Value::Null),
        "inputSchema": tool.get("inputSchema").cloned().unwrap_or(Value::Null),
        "outputSchema": tool.get("outputSchema").cloned().unwrap_or(Value::Null),
    });
    assert_eq!(shape["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        shape["inputSchema"]["properties"]["target"]["default"],
        "observations"
    );
    assert_eq!(
        shape["inputSchema"]["properties"]["format"]["default"],
        "jsonl"
    );
    assert_eq!(
        shape["inputSchema"]["properties"]["target"]["enum"],
        json!(["observations", "events", "both"])
    );
    assert_eq!(
        shape["inputSchema"]["properties"]["format"]["enum"],
        json!(["jsonl"])
    );
    assert_eq!(shape["inputSchema"]["required"], json!(["duration_ms"]));
    assert_eq!(
        shape["outputSchema"]["required"],
        json!(["path", "records_written", "observations_skipped", "bytes"])
    );
    insta::assert_json_snapshot!("m3_replay_record_tool", shape);
}
