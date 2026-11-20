//! #10 regression: an outer Ok(act response) whose nested action has ok=false
//! must classify as a failure for command audit and lifecycle; real successes,
//! lease-only responses and unrelated tools must not.

use serde_json::{Value, json};

use super::{nested_action_failure, nested_failure_in_tool_result};

fn action(ok: bool, status: &str, result: Value) -> Value {
    json!({
        "verb": "click",
        "ok": ok,
        "status": status,
        "delivery_state": if ok { "delivered_and_verified" } else { "refused_before_delivery" },
        "delegated_tool": "act_click",
        "routing": "test",
        "result": result,
    })
}

#[test]
fn must_pass_successful_action_is_not_a_failure() {
    assert_eq!(
        nested_action_failure(true, "ok", "act_click", &json!({})),
        None
    );
}

#[test]
fn must_fail_refused_action_reports_the_delegated_error_code() {
    let result = json!({"error": {"code": "TARGET_CLAIM_NOT_FOUND", "message": "no claim"}});
    let failure = nested_action_failure(false, "refused", "act_click", &result)
        .expect("ok=false must be a failure");
    assert_eq!(failure.code.as_deref(), Some("TARGET_CLAIM_NOT_FOUND"));
    assert!(failure.message.contains("refused"), "{}", failure.message);
    assert!(failure.message.contains("act_click"), "{}", failure.message);
    let snapshot = failure.error_snapshot();
    assert_eq!(
        snapshot.get("synapse_code").and_then(Value::as_str),
        Some("TARGET_CLAIM_NOT_FOUND")
    );
}

#[test]
fn must_fail_failed_verification_without_error_code_is_still_a_failure() {
    let failure = nested_action_failure(false, "verify_needed", "act_type", &json!({"x": 1}))
        .expect("verify_needed with ok=false must be a failure");
    assert_eq!(failure.code, None);
    assert!(failure.message.contains("verify_needed"));
}

#[test]
fn must_fail_ok_true_with_non_ok_status_is_still_a_failure() {
    let failure = nested_action_failure(true, "verify_needed", "act_click", &json!({}))
        .expect("ok=true with status verify_needed must be a failure");
    assert_eq!(failure.code, None);
    assert_eq!(
        failure.message,
        "delegated action act_click did not succeed: ok=true status=verify_needed"
    );
}

#[test]
fn must_fail_act_facade_with_failed_nested_action_is_a_failure() {
    let result = json!({"error": {"code": "ACTION_POSTCONDITION_FAILED"}});
    let response = json!({
        "operation": "invoke",
        "source_of_truth": "act",
        "action": action(false, "error", result),
    });
    let failure = nested_failure_in_tool_result("act", &response).expect("nested failure");
    assert_eq!(failure.code.as_deref(), Some("ACTION_POSTCONDITION_FAILED"));
}

#[test]
fn must_pass_act_facade_with_ok_nested_action_is_not_a_failure() {
    let response = json!({"operation": "invoke", "action": action(true, "ok", json!({}))});
    assert_eq!(nested_failure_in_tool_result("act", &response), None);
}

#[test]
fn must_pass_act_facade_lease_response_without_action_is_not_a_failure() {
    let response = json!({"operation": "lease_status", "lease": {"held": false}});
    assert_eq!(nested_failure_in_tool_result("act", &response), None);
}

#[test]
fn must_fail_act_foreground_nested_action_failure_is_detected() {
    let response = json!({"ok": false, "action": action(false, "refused", json!({}))});
    let failure =
        nested_failure_in_tool_result("act_foreground", &response).expect("nested failure");
    assert_eq!(failure.code, None);
    assert_eq!(
        failure.message,
        "delegated action act_click did not succeed: ok=false status=refused"
    );
}

#[test]
fn must_fail_target_act_top_level_failure_is_detected() {
    let response = action(false, "refused", json!({}));
    let failure = nested_failure_in_tool_result("target_act", &response).expect("nested failure");
    assert_eq!(failure.code, None);
    assert_eq!(
        failure.message,
        "delegated action act_click did not succeed: ok=false status=refused"
    );
}

#[test]
fn must_pass_unrelated_tools_with_ok_false_data_are_not_reclassified() {
    let response = json!({"ok": false, "action": action(false, "refused", json!({}))});
    assert_eq!(nested_failure_in_tool_result("health", &response), None);
}

#[test]
fn must_fail_malformed_action_without_ok_field_is_not_guessed() {
    let response = json!({"operation": "invoke", "action": {"status": "error"}});
    assert_eq!(nested_failure_in_tool_result("act", &response), None);
}
