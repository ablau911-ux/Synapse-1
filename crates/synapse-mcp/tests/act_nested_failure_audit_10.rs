//! #10: an `act` call whose delegated action fails (`action.ok=false`) must be
//! recorded as a failure by BOTH the final command-audit row and the daemon
//! lifecycle ledger, while the caller still receives the structured response.
//!
//! Contract: issue #10 "Required behavior"/"Acceptance"; fix comment
//! ("daemon-tool-events.jsonl records tool=act status=error with synapse_code
//! ...; command_query reads final CF_ACTION_LOG outcome ... error_code ...");
//! server/background_router.rs `act` (nested_action_failure -> final audit
//! outcome "error" + error code); server/handler.rs `call_tool`
//! (`classify_tool_success` -> `finish_error_with_effective_target`).
//!
//! Real path: stdio daemon (schema-validating client) with
//! `SYNAPSE_MCP_FORCE_NO_FOREGROUND=1`, so `act invoke verb=scroll` (no point,
//! needs the foreground) fails inside `act_scroll` with A11Y_NO_FOREGROUND and
//! delivers nothing. Readbacks: `audit operation=command_query` and the
//! physical `<db>/daemon-tool-events.jsonl`. The ok case uses `act
//! lease_status`, which needs an MCP session id, so it runs on the HTTP daemon
//! (`support::HttpMcpSession`, same schema validation as the stdio client).

#[allow(dead_code)]
mod support;

use std::path::Path;

use anyhow::Context;
use serde_json::{Value, json};
use synapse_core::error_codes;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

async fn launch(db: &TempDir) -> anyhow::Result<StdioMcpClient> {
    let db_path = db.path().join("db");
    let db_arg = db_path.to_string_lossy().into_owned();
    StdioMcpClient::launch_and_init_with_args(
        None,
        &["--db", &db_arg],
        &[("SYNAPSE_MCP_FORCE_NO_FOREGROUND", "1")],
    )
    .await
}

fn act_command_query() -> Value {
    json!({"operation": "command_query", "command_query": {"tool": "act", "limit": 50}})
}

/// Final command-audit rows of tool `act`, oldest first.
async fn act_final_audits(client: &mut StdioMcpClient) -> anyhow::Result<Vec<Value>> {
    let response = client.tools_call("audit", act_command_query()).await?;
    final_act_rows(&response)
}

/// Final rows of an `audit command_query` response, oldest first.
fn final_act_rows(response: &Value) -> anyhow::Result<Vec<Value>> {
    let rows = response["structuredContent"]["command_query"]["rows"]
        .as_array()
        .context("command_query rows")?;
    let mut finals: Vec<Value> = rows
        .iter()
        .filter(|row| row["phase"] == "final")
        .cloned()
        .collect();
    finals.sort_by_key(|row| row["ts_ns"].as_u64().unwrap_or_default());
    Ok(finals)
}

/// Completed (non-`started`) lifecycle events of tool `act`, in file order.
fn act_completed_events(db: &TempDir) -> anyhow::Result<Vec<Value>> {
    act_completed_events_at(&db.path().join("db"))
}

/// Completed lifecycle events of tool `act` in `<db_path>/daemon-tool-events.jsonl`.
fn act_completed_events_at(db_path: &Path) -> anyhow::Result<Vec<Value>> {
    let path = db_path.join("daemon-tool-events.jsonl");
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["tool"] == "act" && event["status"] != "started")
        .collect())
}

#[tokio::test]
async fn must_fail_nested_action_failure_is_recorded_as_error_everywhere() -> anyhow::Result<()> {
    let db = TempDir::new()?;
    let mut client = launch(&db).await?;
    assert!(
        act_final_audits(&mut client).await?.is_empty(),
        "precondition: no act rows"
    );

    let result = client
        .tools_call_expect_tool_error(
            "act",
            json!({"operation": "invoke", "action": {"verb": "scroll", "args": ["0", "-1"]}}),
        )
        .await?;

    // The caller still gets the structured response with the nested failure.
    let action = &result["structuredContent"]["action"];
    assert_eq!(action["ok"], false, "{result}");
    assert_eq!(action["delegated_tool"], "act_scroll", "{result}");
    assert_eq!(
        action["result"]["error"]["code"],
        error_codes::A11Y_NO_FOREGROUND,
        "{result}"
    );

    let audits = act_final_audits(&mut client).await?;
    assert_eq!(audits.len(), 1, "one final act audit row: {audits:?}");
    assert_eq!(audits[0]["outcome"], "error", "{}", audits[0]);
    assert_eq!(
        audits[0]["error_code"],
        error_codes::A11Y_NO_FOREGROUND,
        "{}",
        audits[0]
    );

    let events = act_completed_events(&db)?;
    assert_eq!(
        events.len(),
        1,
        "one completed act lifecycle event: {events:?}"
    );
    assert_eq!(events[0]["status"], "error", "{}", events[0]);
    assert_eq!(
        events[0]["error"]["synapse_code"],
        error_codes::A11Y_NO_FOREGROUND,
        "{}",
        events[0]
    );
    assert!(client.shutdown().await?.success());
    Ok(())
}

/// Must NOT change: an `act` call with no failing nested action stays "ok".
/// `act lease_status` requires an MCP session id (server/lease_tools.rs
/// `require_lease_session_id`), which a stdio daemon never has.
#[tokio::test]
async fn must_pass_act_without_nested_failure_is_recorded_ok() -> anyhow::Result<()> {
    let daemon = support::HttpDaemon::start(&[("SYNAPSE_MCP_FORCE_NO_FOREGROUND", "1")]).await?;
    let mut session = daemon.session("act-nested-ok-10", &[]).await?;

    let result = session
        .tools_call("act", json!({"operation": "lease_status"}))
        .await?;
    assert_eq!(
        result["structuredContent"]["operation"], "lease_status",
        "{result}"
    );

    let audits = final_act_rows(&session.tools_call("audit", act_command_query()).await?)?;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0]["outcome"], "ok", "{}", audits[0]);
    assert_eq!(audits[0]["error_code"], Value::Null, "{}", audits[0]);

    let events = act_completed_events_at(daemon.db_path())?;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["status"], "ok", "{}", events[0]);
    assert_eq!(events[0]["error"], Value::Null, "{}", events[0]);
    drop(daemon.stop().await?);
    Ok(())
}
