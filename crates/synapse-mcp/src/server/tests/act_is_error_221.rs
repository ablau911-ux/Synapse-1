//! Regression tests for #221: an `act`/`target_act` call whose nested action
//! was refused or failed must reach MCP clients as `isError=true`, not as a
//! successful tool call.

use rmcp::model::CallToolResult;
use serde_json::json;

use super::mark_nested_failure;

fn act_result(ok: bool, status: &str) -> CallToolResult {
    CallToolResult::structured(json!({
        "operation": "invoke",
        "action": {
            "ok": ok,
            "status": status,
            "delegated_tool": "act_click",
            "result": { "error": { "code": "ACTION_REFUSED" } },
        },
    }))
}

#[test]
fn must_fail_refused_act_is_marked_is_error() {
    let mut result = act_result(false, "refused");
    let failure = mark_nested_failure("act", &mut result).expect("refused act is a nested failure");
    assert_eq!(failure.code.as_deref(), Some("ACTION_REFUSED"));
    assert_eq!(
        failure.message,
        "delegated action act_click did not succeed: ok=false status=refused"
    );
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn must_fail_failed_target_act_is_marked_is_error() {
    let mut result = CallToolResult::structured(json!({
        "ok": false,
        "status": "failed",
        "delegated_tool": "act_click",
        "result": null,
    }));
    assert!(mark_nested_failure("target_act", &mut result).is_some());
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn must_pass_successful_act_stays_not_an_error() {
    let mut result = act_result(true, "ok");
    assert!(mark_nested_failure("act", &mut result).is_none());
    assert_ne!(result.is_error, Some(true));
}

#[test]
fn must_pass_lease_operations_and_other_tools_are_untouched() {
    let mut lease = CallToolResult::structured(json!({ "operation": "lease_status", "lease": {} }));
    assert!(mark_nested_failure("act", &mut lease).is_none());
    assert_ne!(lease.is_error, Some(true));

    let mut other = act_result(false, "refused");
    assert!(mark_nested_failure("observe", &mut other).is_none());
    assert_ne!(other.is_error, Some(true));
}
