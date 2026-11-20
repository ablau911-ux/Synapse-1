use anyhow::{Context, ensure};
use serde_json::{Value, json};
use synapse_core::error_codes;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

#[tokio::test]
async fn must_pass_act_stroke_is_the_unified_public_motion_tool() -> anyhow::Result<()> {
    let log_dir = TempDir::new()?;
    let db = TempDir::new()?;
    let mut client = launch_stroke_client(&log_dir, &db).await?;

    let tools_response = client.tools_list().await?;
    ensure!(
        tool_by_name(&tools_response, "act_aim").is_err()
            && tool_by_name(&tools_response, "act_drag").is_err(),
        "act_aim and act_drag must not be advertised after act_stroke unification"
    );
    let stroke = tool_by_name(&tools_response, "act_stroke")?;
    let description = stroke
        .get("description")
        .and_then(Value::as_str)
        .context("act_stroke description missing")?;
    ensure!(
        description.contains("point/element target")
            && description.contains("button set drags")
            && description.contains("explicit spatial path"),
        "act_stroke description must describe unified target/path/button semantics: {description}"
    );
    for field in ["path", "target", "from", "to", "button"] {
        value_at(stroke, &format!("inputSchema.properties.{field}"))
            .with_context(|| format!("act_stroke schema must advertise {field}"))?;
    }
    ensure!(
        value_at(stroke, "inputSchema.properties.curve").is_err(),
        "act_stroke schema must not advertise the old act_drag curve alias"
    );

    // The live drag behaviour of the unified tool is covered by
    // m2_act_stroke_tool target_line; this test owns the surface shape only.

    assert!(client.shutdown().await?.success());
    Ok(())
}

#[tokio::test]
async fn must_fail_act_stroke_rejects_the_legacy_act_drag_curve_alias() -> anyhow::Result<()> {
    let log_dir = TempDir::new()?;
    let db = TempDir::new()?;
    let mut client = launch_stroke_client(&log_dir, &db).await?;

    let legacy_curve = client
        .tools_call_error(
            "act_stroke",
            json!({
                "from": {"x": 10.0, "y": 20.0},
                "to": {"x": 70.0, "y": 80.0},
                "curve": "bezier",
                "duration_or_speed": {"kind": "duration_ms", "duration_ms": 80},
                "backend": "software"
            }),
        )
        .await?;
    println!("readback=mcp_act_stroke edge=legacy_curve_rejected after_error={legacy_curve}");
    assert_eq!(
        error_code(&legacy_curve),
        Some(error_codes::TOOL_PARAMS_INVALID)
    );
    let message = legacy_curve["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("unknown field") && message.contains("curve"),
        "refusal must name the unknown legacy field `curve`: {legacy_curve}"
    );

    // The retired tool names are gone from the callable surface too.
    for retired in ["act_aim", "act_drag"] {
        let error = client
            .tools_call_error(
                retired,
                json!({
                    "from": {"x": 10.0, "y": 20.0},
                    "to": {"x": 70.0, "y": 80.0}
                }),
            )
            .await?;
        assert_eq!(
            error_code(&error),
            Some(error_codes::TOOL_NOT_FOUND),
            "{retired}: {error}"
        );
    }

    assert!(client.shutdown().await?.success());
    // No side effect: nothing reached the recording backend.
    let mut logs = String::new();
    for entry in std::fs::read_dir(log_dir.path())? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            logs.push_str(&std::fs::read_to_string(entry.path())?);
        }
    }
    assert!(
        !logs.contains("readback=recording_backend tool=act_stroke"),
        "a refused stroke must not be recorded: {logs}"
    );
    Ok(())
}

async fn launch_stroke_client(log_dir: &TempDir, db: &TempDir) -> anyhow::Result<StdioMcpClient> {
    let db_path_string = db.path().join("db").to_string_lossy().to_string();
    let mut client = StdioMcpClient::launch_and_init_with_env(
        Some(log_dir.path()),
        &[
            ("SYNAPSE_DB", db_path_string.as_str()),
            ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
            ("SYNAPSE_MCP_RECORDING_BACKEND", "1"),
            ("SYNAPSE_DEBUG_TOOLS", "1"),
        ],
    )
    .await?;
    client
        .tools_call("profile_activate", json!({"profile_id": "notepad"}))
        .await?;
    Ok(client)
}

fn tool_by_name<'a>(tools_response: &'a Value, name: &str) -> anyhow::Result<&'a Value> {
    tools_response
        .get("tools")
        .and_then(Value::as_array)
        .context("tools array missing")?
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
        .with_context(|| format!("tool missing from tools/list: {name}"))
}

fn value_at<'a>(value: &'a Value, path: &str) -> anyhow::Result<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current
            .get(segment)
            .with_context(|| format!("missing path {path}"))?;
    }
    Ok(current)
}

fn error_code(error: &Value) -> Option<&str> {
    error
        .get("data")
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}
