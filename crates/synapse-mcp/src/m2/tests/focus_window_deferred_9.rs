//! #9 regression: a Windows `SetForegroundWindow` refusal (including the lock
//! screen / secure input desktop) must reach the caller as a typed, retryable
//! deferral that names the preserved target, never as a generic internal error.

use serde_json::{Value, json};
use synapse_core::error_codes;

use super::a11y_error_to_focus_mcp;

fn data(error: &rmcp::ErrorData) -> &Value {
    error
        .data
        .as_ref()
        .expect("focus_window errors always carry structured data")
}

fn prior_readback() -> Value {
    json!({ "matched_window": { "hwnd": 16_518_336_i64, "pid": 4242 }, "attempts": 1 })
}

#[test]
fn must_pass_activation_refusal_is_typed_retryable_and_names_the_target() {
    let refusal = synapse_a11y::A11yError::foreground_activation_refused(
        "SetForegroundWindow returned false for hwnd 0xfc0cc0",
    );
    let error = a11y_error_to_focus_mcp("set_foreground_window", refusal, Some(prior_readback()));
    let data = data(&error);
    assert_eq!(
        data["code"],
        error_codes::FOREGROUND_ACTIVATION_REFUSED,
        "{data}"
    );
    assert_eq!(data["retryable"], true, "{data}");
    assert_eq!(
        data["retry_after_ms"].as_u64(),
        Some(u64::from(
            crate::m2::foreground_deferral::FOREGROUND_RETRY_AFTER_MS
        )),
        "retry_after_ms must be the documented deferral hint: {data}"
    );
    assert!(
        data["lock_screen"].is_boolean(),
        "lock-screen classification must be reported either way: {data}"
    );
    assert_eq!(
        data["target_hwnd"], 16_518_336_i64,
        "the deferral must keep the target: {data}"
    );
}

#[test]
fn must_fail_internal_failures_are_not_marked_retryable() {
    let internal = synapse_a11y::A11yError::internal("boom");
    let error = a11y_error_to_focus_mcp("set_foreground_window", internal, Some(prior_readback()));
    let data = data(&error);
    assert_eq!(data["code"], error_codes::OBSERVE_INTERNAL, "{data}");
    assert_ne!(data["retryable"], true, "{data}");
}

#[test]
fn must_fail_refusal_outside_activation_is_not_deferred() {
    // A refusal raised while merely reading the prior foreground is a policy
    // answer, not a transient activation failure.
    let refusal = synapse_a11y::A11yError::foreground_activation_refused("policy");
    let error = a11y_error_to_focus_mcp("before_foreground_read", refusal, None);
    let data = data(&error);
    assert_eq!(
        data["code"],
        error_codes::FOREGROUND_ACTIVATION_REFUSED,
        "{data}"
    );
    assert_ne!(data["retryable"], true, "{data}");
    assert!(data.get("retry_after_ms").is_none(), "{data}");
    assert!(data.get("target_hwnd").is_none(), "{data}");
    assert!(data.get("deferred_reason").is_none(), "{data}");
}
