//! #10: classify a delegated `act`/`target_act` result whose outer MCP call
//! succeeded but whose nested action did not (`ok=false`: refused, error,
//! verify_needed). Command audit and the daemon lifecycle ledger use this so a
//! failed action is never recorded as a success.

use rmcp::model::CallToolResult;
use serde_json::{Value, json};

/// A nested action failure behind an outer `Ok` tool result.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NestedActionFailure {
    pub code: Option<String>,
    pub message: String,
    pub data: Value,
}

impl NestedActionFailure {
    /// Lifecycle-ledger error snapshot (same shape as `handler::error_snapshot`).
    pub(super) fn error_snapshot(&self) -> Value {
        json!({
            "rmcp_code": Value::Null,
            "message": self.message,
            "data": self.data,
            "synapse_code": self.code,
        })
    }
}

/// Failure description for a delegated action, or `None` when it succeeded.
pub(super) fn nested_action_failure(
    ok: bool,
    status: &str,
    delegated_tool: &str,
    result: &Value,
) -> Option<NestedActionFailure> {
    if ok && status == "ok" {
        return None;
    }
    let code = result
        .pointer("/error/code")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(NestedActionFailure {
        message: format!(
            "delegated action {delegated_tool} did not succeed: ok={ok} status={status}"
        ),
        data: json!({
            "delegated_tool": delegated_tool,
            "action_ok": ok,
            "action_status": status,
            "code": code,
            "delegated_error": result.get("error"),
        }),
        code,
    })
}

/// Nested failure in an action response or an active facade operation.
pub(super) fn nested_failure_in_tool_result(
    tool_name: &str,
    structured: &Value,
) -> Option<NestedActionFailure> {
    let action = match tool_name {
        "act" | "act_foreground" => structured.get("action")?,
        "target_act" => structured,
        _ if structured
            .get("operation")
            .and_then(Value::as_str)
            .is_some()
            || matches!(
                tool_name,
                "browser_batch" | "browser_fill_form" | "browser_set_value"
            ) =>
        {
            return facade_operation_failure(tool_name, structured);
        }
        _ => return None,
    };
    let ok = action.get("ok").and_then(Value::as_bool)?;
    let status = action
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let delegated_tool = action
        .get("delegated_tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    nested_action_failure(
        ok,
        status,
        delegated_tool,
        action.get("result").unwrap_or(&Value::Null),
    )
}

/// Inspect response envelopes, not arbitrary page values, audit history or
/// caller data nested inside a successful result.
fn facade_operation_failure(tool_name: &str, structured: &Value) -> Option<NestedActionFailure> {
    if structured.get("ok").and_then(Value::as_bool) == Some(false) {
        let status = structured
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("error");
        return nested_action_failure(false, status, tool_name, structured);
    }
    // browser_debugger.reload_bridge wraps the command outcome in its
    // command_ack readback rather than exposing an aggregate ok.
    if let Some(ack) = structured.get("command_ack")
        && let Some(failure) = facade_operation_failure(tool_name, ack)
    {
        return Some(failure);
    }
    // Facades store their delegate under the operation's snake_case name:
    // browser_form.fill, routine.automate, model.probe, etc.
    let operation = structured.get("operation").and_then(Value::as_str)?;
    let delegate = structured.get(operation)?;
    if delegate.is_null() {
        return None;
    }
    facade_operation_failure(tool_name, delegate)
}

/// Nested failure inside `result` (see [`nested_failure_in_tool_result`]); when
/// there is one, also flags the MCP result `isError=true` so clients do not
/// treat a refused or failed action as a successful call (#221).
pub(super) fn mark_nested_failure(
    tool_name: &str,
    result: &mut CallToolResult,
) -> Option<NestedActionFailure> {
    let failure = result
        .structured_content
        .as_ref()
        .and_then(|value| nested_failure_in_tool_result(tool_name, value))?;
    result.is_error = Some(true);
    Some(failure)
}

#[cfg(test)]
#[path = "tests/act_outcome_10.rs"]
mod act_outcome_10;

#[cfg(test)]
#[path = "tests/act_is_error_221.rs"]
mod act_is_error_221;
