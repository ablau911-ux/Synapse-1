//! #9 regression: a retryable foreground refusal reaches `target_act` as
//! status=deferred / delivery_state=deferred_without_delivery with the target
//! preserved for retry, while real policy refusals and internal errors keep
//! their existing classification.

use rmcp::model::ErrorCode;
use serde_json::{Value, json};

use super::*;

fn refusal(extra: Value) -> ErrorData {
    let mut data = json!({
        "code": error_codes::FOREGROUND_ACTIVATION_REFUSED,
        "operation": "set_foreground_window",
        "prior_readback": { "matched_window": { "hwnd": 16_518_336_i64 } },
    });
    if let (Some(base), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    ErrorData::new(ErrorCode(-32099), "foreground refused", Some(data))
}

fn deferrable() -> ErrorData {
    refusal(json!({
        "retryable": true,
        "retry_after_ms": 1000,
        "lock_screen": true,
        "target_hwnd": 16_518_336_i64,
    }))
}

#[test]
fn must_pass_retryable_foreground_refusal_is_status_deferred() {
    assert_eq!(target_act_error_status(&deferrable()), "deferred");
}

#[test]
fn must_fail_deferred_status_means_nothing_was_delivered() {
    assert_eq!(
        DeliveryClaim::without_result(false, "deferred").state(),
        "deferred_without_delivery"
    );
}

#[test]
fn must_pass_deferred_result_exposes_retry_hint_and_preserved_target() {
    let result = target_act_error_result("act_focus_window", deferrable());
    assert_eq!(result["retryable"], true, "{result}");
    assert_eq!(result["retry_after_ms"], 1000, "{result}");
    assert_eq!(result["continuation"]["preserved"], true, "{result}");
    assert_eq!(
        result["continuation"]["target_hwnd"], 16_518_336_i64,
        "{result}"
    );
    assert_eq!(
        result["error"]["code"],
        error_codes::FOREGROUND_ACTIVATION_REFUSED,
        "{result}"
    );
}

#[test]
fn must_fail_non_retryable_foreground_refusal_stays_refused() {
    let policy = refusal(json!({}));
    assert_eq!(target_act_error_status(&policy), "refused");
    let result = target_act_error_result("act_focus_window", policy);
    assert!(result.get("retryable").is_none(), "{result}");
    assert!(result.get("continuation").is_none(), "{result}");
}

#[test]
fn must_fail_internal_error_stays_error() {
    let internal = mcp_error(error_codes::OBSERVE_INTERNAL, "boom");
    assert_eq!(target_act_error_status(&internal), "error");
}

#[test]
fn must_fail_deferred_response_for_the_foreground_lane_is_not_ok_and_keeps_the_verb() {
    let response = target_act_deferred_response("key", deferrable());
    assert!(!response.ok);
    assert_eq!(response.status, "deferred");
    assert_eq!(response.delivery_state, "deferred_without_delivery");
    assert_eq!(response.verb, "key");
    assert_eq!(response.result["retryable"], true);
}
