//! #95 second review: the operator-hotkey `release_all` budget parser, the
//! 50 ms K1 target reporting, and the mapping from the emitter outcome (primary
//! ack plus secondary drains) to the K1 report.

use std::time::Duration;

use synapse_action::{ActionError, ReleaseAllCompletion, ReleaseAllOutcome};

use super::{
    K1_RELEASE_ALL_TARGET, OPERATOR_RELEASE_ALL_BUDGET_ENV, OPERATOR_RELEASE_ALL_COMPLETION_ENV,
    parse_release_all_budget, parse_release_all_completion_wait, release_all_fire_from_outcome,
    within_k1_target,
};

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn parsed(raw: Option<&str>) -> Duration {
    parse_release_all_budget(raw).unwrap_or_else(|error| panic!("{raw:?} should parse: {error}"))
}

#[test]
fn must_pass_budget_defaults_to_the_documented_target_when_unset_or_blank() {
    assert_eq!(parsed(None), ms(50));
    assert_eq!(parsed(Some("")), ms(50));
    assert_eq!(parsed(Some("   ")), ms(50));
}

#[test]
fn must_pass_budget_accepts_trimmed_positive_milliseconds() {
    assert_eq!(parsed(Some("25")), ms(25));
    assert_eq!(parsed(Some(" 75 ")), ms(75));
    assert_eq!(parsed(Some("1000")), ms(1000));
}

#[test]
fn must_fail_budget_rejects_zero_negative_fractional_and_garbage() {
    for raw in ["0", "-5", "1.5", "abc", "50ms", "0x10"] {
        let detail = refusal_detail(parse_release_all_budget(Some(raw)), raw);
        assert!(
            detail.contains(OPERATOR_RELEASE_ALL_BUDGET_ENV),
            "{raw:?}: detail must name the budget env var: {detail}"
        );
        assert!(
            detail.contains("positive integer number of milliseconds"),
            "{raw:?}: {detail}"
        );
    }
}

/// The `BackendUnavailable` detail of a refused wait value; any other outcome
/// fails the test.
fn refusal_detail(result: Result<Duration, ActionError>, raw: &str) -> String {
    match result {
        Err(ActionError::BackendUnavailable { detail }) => detail,
        other => panic!("{raw:?} must be refused as BackendUnavailable, got {other:?}"),
    }
}

#[test]
fn must_pass_budget_above_max_is_clamped_to_1000ms() {
    assert_eq!(parsed(Some("1001")), ms(1000));
    assert_eq!(parsed(Some("999999999")), ms(1000));
    assert_eq!(parsed(Some("18446744073709551615")), ms(1000));
}

#[test]
fn must_pass_completion_wait_has_its_own_default_and_the_same_clamp() {
    let default = parse_release_all_completion_wait(None)
        .unwrap_or_else(|error| panic!("default should parse: {error}"));
    assert_eq!(default, ms(250));
    let clamped = parse_release_all_completion_wait(Some("86400000"))
        .unwrap_or_else(|error| panic!("huge value should clamp: {error}"));
    assert_eq!(clamped, ms(1000));
}

#[test]
fn must_fail_completion_wait_rejects_zero_and_garbage() {
    for raw in ["0", "soon"] {
        let detail = refusal_detail(parse_release_all_completion_wait(Some(raw)), raw);
        assert!(
            detail.contains(OPERATOR_RELEASE_ALL_COMPLETION_ENV),
            "{raw:?}: detail must name the completion env var: {detail}"
        );
        assert!(
            !detail.contains(OPERATOR_RELEASE_ALL_BUDGET_ENV),
            "{raw:?}: detail must not blame the budget env var: {detail}"
        );
    }
}

#[test]
fn must_pass_within_budget_is_judged_against_the_50ms_target_not_the_configured_timeout() {
    assert_eq!(K1_RELEASE_ALL_TARGET, ms(50));
    assert!(within_k1_target(ms(0)));
    assert!(within_k1_target(ms(50)));
    assert!(!within_k1_target(ms(51)));
    // A 1000 ms configured timeout must not turn a 400 ms release into "within budget".
    assert!(!within_k1_target(ms(400)));
}

fn outcome(
    primary: Result<(), ActionError>,
    completion: ReleaseAllCompletion,
) -> ReleaseAllOutcome {
    ReleaseAllOutcome {
        primary,
        primary_ack_after: ms(7),
        completion,
    }
}

fn unavailable(detail: &str) -> ActionError {
    ActionError::BackendUnavailable {
        detail: detail.to_owned(),
    }
}

#[test]
fn must_pass_fully_released_outcome_reports_ok() {
    let fire =
        release_all_fire_from_outcome(Ok(outcome(Ok(()), ReleaseAllCompletion::Complete(Ok(())))));
    assert_eq!(fire.report.result, "ok");
    assert!(fire.report.error_code.is_none());
    assert_eq!(fire.primary_ack_after, Some(ms(7)));
}

#[test]
fn must_fail_failed_secondary_drain_is_reported_as_partial_failure() {
    let fire = release_all_fire_from_outcome(Ok(outcome(
        Ok(()),
        ReleaseAllCompletion::Complete(Err(unavailable("vigem drain failed"))),
    )));
    assert_eq!(fire.report.result, "partial_failure");
    assert_eq!(
        fire.report.error_code,
        Some(synapse_core::error_codes::ACTION_BACKEND_UNAVAILABLE)
    );
    let detail = fire.report.detail.unwrap_or_default();
    assert!(detail.contains("vigem drain failed"), "detail={detail}");
    assert_eq!(fire.primary_ack_after, Some(ms(7)));
}

#[test]
fn must_fail_secondary_drain_still_running_is_reported_as_pending_not_ok() {
    let fire = release_all_fire_from_outcome(Ok(outcome(Ok(()), ReleaseAllCompletion::Pending)));
    assert_eq!(fire.report.result, "ok_secondary_pending");
    assert!(fire.report.error_code.is_none());
    assert_eq!(fire.primary_ack_after, Some(ms(7)));
}

#[test]
fn must_fail_primary_failure_is_an_error() {
    let fire = release_all_fire_from_outcome(Ok(outcome(
        Err(unavailable("software release failed")),
        ReleaseAllCompletion::Complete(Err(unavailable("software release failed"))),
    )));
    assert_eq!(fire.report.result, "error");
    assert_eq!(
        fire.report.error_code,
        Some(synapse_core::error_codes::ACTION_BACKEND_UNAVAILABLE)
    );
    let detail = fire.report.detail.unwrap_or_default();
    assert!(
        detail.contains("software release failed"),
        "detail={detail}"
    );
    assert_eq!(fire.primary_ack_after, Some(ms(7)));
}

#[test]
fn must_fail_ack_timeout_or_enqueue_failure_is_an_error_without_a_primary_ack_time() {
    let fire = release_all_fire_from_outcome(Err(unavailable("acknowledgement timed out")));
    assert_eq!(fire.report.result, "error");
    assert_eq!(
        fire.report.error_code,
        Some(synapse_core::error_codes::ACTION_BACKEND_UNAVAILABLE)
    );
    let detail = fire.report.detail.unwrap_or_default();
    assert!(
        detail.contains("acknowledgement timed out"),
        "detail={detail}"
    );
    assert_eq!(fire.primary_ack_after, None);
}
