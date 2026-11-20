//! `subscribe` is a session-owned facade (`operation=events|events_cancel`).
//! Over stdio there is no MCP session, so the lifecycle (create, cancel, cap)
//! cannot be driven here; this test pins the advertised schema and the
//! fail-closed refusal for ownerless callers.

use anyhow::Context;
use serde_json::{Value, json};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

#[tokio::test]
async fn must_pass_subscribe_schema_advertises_operations_and_defaults() -> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let mut client = StdioMcpClient::launch_and_init_with_log_dir(Some(logs.path())).await?;

    let tools = client.tools_list().await?;
    let tools = tools
        .get("tools")
        .and_then(Value::as_array)
        .context("tools array missing")?;
    let subscribe_tool = tools
        .iter()
        .find(|tool| tool["name"] == "subscribe")
        .context("subscribe tool missing")?;
    assert_subscribe_schema(subscribe_tool);

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn must_fail_subscribe_ownerless_missing_payload_and_unknown_operation_are_refused()
-> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let mut client = StdioMcpClient::launch_and_init_with_log_dir(Some(logs.path())).await?;

    let ownerless = client
        .tools_call_error("subscribe", json!({"operation": "events", "events": {}}))
        .await?;
    assert_eq!(ownerless["data"]["code"], "TOOL_PARAMS_INVALID");
    assert!(
        ownerless["message"]
            .as_str()
            .is_some_and(|message| message.contains("trusted MCP session owner")),
        "unexpected ownerless refusal: {ownerless}"
    );

    let missing_payload = client
        .tools_call_error("subscribe", json!({"operation": "events"}))
        .await?;
    assert_eq!(missing_payload["data"]["code"], "TOOL_PARAMS_INVALID");
    // The payload rule runs before (and independently of) the owner gate.
    assert!(
        missing_payload["message"].as_str().is_some_and(
            |message| message.contains("subscribe requires exactly one operation payload")
        ),
        "missing payload must be refused by the payload rule: {missing_payload}"
    );

    let unknown_operation = client
        .tools_call_error("subscribe", json!({"operation": "list"}))
        .await?;
    assert_eq!(unknown_operation["data"]["code"], "TOOL_PARAMS_INVALID");
    assert!(
        unknown_operation["message"]
            .as_str()
            .is_some_and(|message| message.contains("unknown variant `list`")),
        "unknown operation must be refused by name: {unknown_operation}"
    );

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

fn assert_subscribe_schema(subscribe_tool: &Value) {
    let shape = json!({
        "name": subscribe_tool.get("name").cloned().unwrap_or(Value::Null),
        "inputSchema": subscribe_tool.get("inputSchema").cloned().unwrap_or(Value::Null),
        "outputSchema": subscribe_tool.get("outputSchema").cloned().unwrap_or(Value::Null),
    });
    let input = &shape["inputSchema"];
    assert_eq!(input["additionalProperties"], false);
    assert_eq!(
        input["$defs"]["SubscribeOperation"]["enum"],
        json!(["events", "events_cancel"])
    );
    let params = &input["$defs"]["SubscribeParams"]["properties"];
    assert_eq!(params["kinds"]["default"], json!([]));
    assert_eq!(params["snapshot_first"]["default"], false);
    assert_eq!(params["buffer_size"]["default"], 4096);
    insta::assert_json_snapshot!("m3_subscribe_tool", shape);
}
