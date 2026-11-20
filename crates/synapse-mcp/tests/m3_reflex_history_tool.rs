use anyhow::Context;
use serde_json::{Value, json};
use synapse_core::{ReflexState, SCHEMA_VERSION, StoredReflexAudit, error_codes};
use synapse_reflex::REFLEX_TICK_LATE_KIND;
use synapse_storage::{Db, cf, decode_json};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn must_pass_reflex_history_schema_defaults_and_audit_boundaries() -> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let db = TempDir::new()?;
    let db_path = db.path().join("db");
    let mut client = launch_history_client(&logs, &db_path).await?;

    let tools = client.tools_list().await?;
    let tools = tools
        .get("tools")
        .and_then(Value::as_array)
        .context("tools array missing")?;
    let reflex_history_tool = tools
        .iter()
        .find(|tool| tool["name"] == "reflex_history")
        .context("reflex_history tool missing")?;
    assert_reflex_history_schema(reflex_history_tool);

    let empty = client.tools_call("reflex_history", json!({})).await?;
    assert!(
        structured(&empty)?["events"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    let first = register(&mut client, "support-history-a").await?;
    let second = register(&mut client, "support-history-b").await?;
    let cancelled = client
        .tools_call("reflex_cancel", json!({"reflex_id": first}))
        .await?;
    assert_eq!(structured(&cancelled)?["cancelled"], true);

    let filtered = client
        .tools_call("reflex_history", json!({"reflex_id": first, "limit": 2}))
        .await?;
    let filtered = structured(&filtered)?;
    let filtered_events = filtered["events"]
        .as_array()
        .context("filtered events should be an array")?;
    assert_eq!(filtered_events.len(), 2);
    assert!(
        filtered_events
            .iter()
            .all(|event| event["reflex_id"] == first)
    );
    assert_eq!(filtered_events[0]["status"], "cancelled");
    assert_eq!(filtered_events[1]["status"], "active");

    let newest_global = client
        .tools_call("reflex_history", json!({"limit": 1}))
        .await?;
    let newest_global = events(&structured(&newest_global)?)?;
    let [newest] = newest_global.as_slice() else {
        anyhow::bail!("limit 1 must return one event: {newest_global:?}");
    };
    if is_scheduler_tick_late(newest) {
        // A late scheduler tick audited after the cancel is the newest row.
        let cancelled_at = filtered_events[0]["ts_ns"]
            .as_u64()
            .context("cancel row ts_ns")?;
        assert!(
            newest.ts_ns > cancelled_at,
            "newest-first: {newest:?} is older than the cancel row"
        );
    } else {
        assert_eq!(newest.reflex_id, first);
        assert_eq!(newest.status, ReflexState::Cancelled);
    }

    let unknown = client
        .tools_call(
            "reflex_history",
            json!({"reflex_id": "support-history-unknown", "limit": 5}),
        )
        .await?;
    assert!(
        structured(&unknown)?["events"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    let zero = client
        .tools_call("reflex_history", json!({"limit": 0}))
        .await?;
    assert!(
        structured(&zero)?["events"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    let capped = client
        .tools_call("reflex_history", json!({"limit": 1000}))
        .await?;
    assert_eq!(own_rows(&events(&structured(&capped)?)?).len(), 3);

    let status = client.shutdown().await?;
    assert!(status.success());

    let audits = read_audits(&db_path)?;
    assert_eq!(own_rows(&audits).len(), 3);
    assert!(audits.iter().any(|audit| audit.reflex_id == second));
    assert!(audits.iter().any(|audit| {
        audit.reflex_id == first
            && audit.status == ReflexState::Cancelled
            && audit.details["kind"] == "reflex_cancelled"
    }));

    Ok(())
}

#[tokio::test]
async fn must_fail_reflex_history_over_cap_limit_and_blank_id_are_refused() -> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let db = TempDir::new()?;
    let mut client = launch_history_client(&logs, &db.path().join("db")).await?;

    let over_cap = client
        .tools_call_error("reflex_history", json!({"limit": 1001}))
        .await?;
    assert_eq!(over_cap["data"]["code"], "TOOL_PARAMS_INVALID");

    let empty_id = client
        .tools_call_error("reflex_history", json!({"reflex_id": "   "}))
        .await?;
    assert_eq!(empty_id["data"]["code"], "TOOL_PARAMS_INVALID");

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

async fn launch_history_client(
    logs: &TempDir,
    db_path: &std::path::Path,
) -> anyhow::Result<StdioMcpClient> {
    let db_path_string = db_path.to_string_lossy().into_owned();
    let mut client = StdioMcpClient::launch_and_init_with_env(
        Some(logs.path()),
        // Synthetic notepad foreground so the reflex_register supported-use gate
        // does not depend on a real focused window (A11Y_NO_FOREGROUND otherwise).
        &[
            ("SYNAPSE_DB", db_path_string.as_str()),
            ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
        ],
    )
    .await?;
    activate_notepad_profile(&mut client).await?;
    Ok(client)
}

async fn register(client: &mut StdioMcpClient, kind: &str) -> anyhow::Result<String> {
    let response = client
        .tools_call("reflex_register", valid_register_args(kind))
        .await?;
    let payload = structured(&response)?;
    payload["reflex_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .context("reflex_id missing")
}

async fn activate_notepad_profile(client: &mut StdioMcpClient) -> anyhow::Result<()> {
    let response = client
        .tools_call("profile_activate", json!({"profile_id": "notepad"}))
        .await?;
    assert_eq!(structured(&response)?["active_profile_id"], "notepad");
    Ok(())
}

fn valid_register_args(kind: &str) -> Value {
    json!({
        "kind": "on_event",
        "when": {"op": "kind", "kind": kind},
        "then": {"kind": "action", "action": {"kind": "release_all"}}
    })
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

fn events(payload: &Value) -> anyhow::Result<Vec<StoredReflexAudit>> {
    serde_json::from_value(payload["events"].clone()).context("events should be audit rows")
}

/// The scheduler's late-tick diagnostic: written whenever the PC delays its
/// 1 ms timer, so a busy PC adds these rows next to the test's own (#2399 G16).
fn is_scheduler_tick_late(row: &StoredReflexAudit) -> bool {
    row.reflex_id == "__scheduler__"
        && row.error_code.as_deref() == Some(error_codes::REFLEX_TICK_LATE)
        && row.details["kind"] == REFLEX_TICK_LATE_KIND
}

/// The rows of this test's reflexes; anything else must be a late-tick row.
fn own_rows(rows: &[StoredReflexAudit]) -> Vec<&StoredReflexAudit> {
    rows.iter()
        .filter(|row| !is_scheduler_tick_late(row))
        .collect()
}

fn read_audits(db_path: &std::path::Path) -> anyhow::Result<Vec<StoredReflexAudit>> {
    let db = Db::open(db_path, SCHEMA_VERSION)?;
    db.scan_cf(cf::CF_REFLEX_AUDIT)?
        .into_iter()
        .map(|(_key, value)| decode_json::<StoredReflexAudit>(&value).map_err(Into::into))
        .collect()
}

fn assert_reflex_history_schema(tool: &Value) {
    let shape = json!({
        "name": tool.get("name").cloned().unwrap_or(Value::Null),
        "inputSchema": tool.get("inputSchema").cloned().unwrap_or(Value::Null),
        "outputSchema": tool.get("outputSchema").cloned().unwrap_or(Value::Null),
    });
    assert_eq!(shape["inputSchema"]["additionalProperties"], false);
    assert_eq!(shape["inputSchema"]["properties"]["limit"]["default"], 50);
    assert_eq!(shape["inputSchema"]["properties"]["limit"]["maximum"], 1000);
    assert_eq!(shape["outputSchema"]["required"][0], "events");
    insta::assert_json_snapshot!("m3_reflex_history_tool", shape);
}
