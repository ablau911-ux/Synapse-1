use std::time::Duration;

use super::{ACT_FOREGROUND_DEADLINE_SLACK_MS, act_foreground_deadline, run_within_deadline};
use crate::server::tool_profiles::normalize_reason;

#[test]
fn must_pass_deadline_is_lease_ttl_plus_slack() {
    assert_eq!(
        act_foreground_deadline(30_000),
        Duration::from_millis(30_000 + ACT_FOREGROUND_DEADLINE_SLACK_MS)
    );
}

#[test]
fn must_pass_deadline_saturates_at_u64_max() {
    assert_eq!(
        act_foreground_deadline(u64::MAX),
        Duration::from_millis(u64::MAX)
    );
}

#[tokio::test]
async fn must_fail_hung_action_is_abandoned_at_the_deadline() {
    let started = std::time::Instant::now();
    let result: Result<(), _> = run_within_deadline(
        Duration::from_millis(50),
        std::future::pending::<Result<(), rmcp::ErrorData>>(),
    )
    .await;
    let error = result.expect_err("a never-returning action must time out");
    let code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(
        code,
        Some(synapse_core::error_codes::TOOL_INTERNAL_ERROR),
        "a timed-out escalated action is a typed internal error: {error:?}"
    );
    assert!(
        error.message.contains("50 ms deadline"),
        "the refusal must name the exact deadline that elapsed: {}",
        error.message
    );
    assert!(
        error.message.contains("act operation=foreground"),
        "the refusal must name the escalated operation: {}",
        error.message
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn must_pass_action_finishing_in_time_returns_its_own_result() {
    let ok = run_within_deadline(Duration::from_secs(5), async {
        Ok::<_, rmcp::ErrorData>(7_u8)
    })
    .await;
    assert_eq!(ok.expect("in-time action result"), 7);
}

/// The escalation's gate is an explicit non-empty reason, validated before any
/// lease or profile state changes; a blank reason never reaches acquisition.
#[test]
fn must_fail_blank_reason_is_rejected_before_any_escalation_state_changes() {
    assert_eq!(
        normalize_reason(Some("   ")).expect("blank is not an error"),
        None
    );
    assert_eq!(
        normalize_reason(None).expect("absent is not an error"),
        None
    );
    assert_eq!(
        normalize_reason(Some(" needs shift-select ")).expect("reason"),
        Some("needs shift-select".to_owned())
    );
}
