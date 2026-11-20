//! #221 through the REAL MCP handler: an `act` call whose delegated action was
//! refused must reach the client as `isError: true`.
//!
//! Driven through the shared schema-validating HTTP harness (D1 client
//! parity) against an isolated daemon. The refused action is `verb=read` on a
//! session with no bound target, so nothing is observed or delivered.

#[allow(dead_code)]
mod support;

use anyhow::{Context, ensure};
use serde_json::{Value, json};
use support::HttpDaemon;

const IS_ERROR_MARKER: &str = "returned isError=true: ";

fn refused_read() -> Value {
    json!({ "operation": "invoke", "action": { "verb": "read" } })
}

/// The harness refuses an `isError` result and quotes it in the error text;
/// this recovers that MCP result for exact assertions.
fn tool_error_result(error: &anyhow::Error) -> anyhow::Result<Value> {
    let text = format!("{error:#}");
    let start = text
        .find(IS_ERROR_MARKER)
        .with_context(|| format!("expected an isError=true tool result, got: {text}"))?;
    serde_json::from_str(&text[start + IS_ERROR_MARKER.len()..])
        .with_context(|| format!("isError result is not JSON: {text}"))
}

fn assert_refused_read(result: &Value) -> anyhow::Result<()> {
    ensure!(
        result["isError"] == json!(true),
        "isError must be true: {result}"
    );
    let action = &result["structuredContent"]["action"];
    ensure!(action["ok"] == json!(false), "action.ok: {result}");
    ensure!(
        action["status"] == json!("refused"),
        "action.status: {result}"
    );
    ensure!(
        action["delivery_state"] == json!("refused_before_delivery"),
        "action.delivery_state: {result}"
    );
    ensure!(
        action["delegated_tool"] == json!("observe"),
        "delegated_tool: {result}"
    );
    ensure!(
        action["result"]["error"]["code"] == json!(synapse_core::error_codes::TARGET_NOT_SET),
        "nested error code: {result}"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_refused_act_reaches_the_client_as_is_error() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("syn221-refused", &[]).await?;

    let first = session
        .tools_call("act", refused_read())
        .await
        .expect_err("a refused act must not be a successful tool call");
    assert_refused_read(&tool_error_result(&first)?)?;

    // No side effect: the refusal did not bind a target, so it repeats identically.
    let second = session
        .tools_call("act", refused_read())
        .await
        .expect_err("still refused");
    assert_refused_read(&tool_error_result(&second)?)?;

    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_successful_act_lease_status_is_not_an_error() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("syn221-lease-status", &[]).await?;

    let result = session
        .tools_call("act", json!({ "operation": "lease_status" }))
        .await?;

    ensure!(
        result.get("isError").and_then(Value::as_bool) != Some(true),
        "{result}"
    );
    let content = &result["structuredContent"];
    ensure!(content["operation"] == json!("lease_status"), "{result}");
    ensure!(content["action"].is_null(), "no delegated action: {result}");
    ensure!(
        content["lease"].is_object(),
        "lease readback present: {result}"
    );

    drop(daemon.stop().await?);
    Ok(())
}
