//! #190: the delivered attempt that `reconcile_click_postdispatch_error` records
//! copies the tier, foreground use and `foreground_authorization` of the attempt
//! it reconciles, so a claimed foreground route never turns into an unstamped
//! foreground attempt the audit would flag as a policy violation.

use super::m2_tools_source_scan::method_body;
use super::*;

fn postdispatch_error() -> ErrorData {
    mcp_error(
        error_codes::TOOL_INTERNAL_ERROR,
        "post-dispatch readback failed",
    )
}

fn claimed_foreground_attempt() -> ActClickTierAttempt {
    let mut attempt = click_tier_failed(
        CLICK_TIER_FOREGROUND,
        "postdispatch_readback_failed",
        error_codes::TOOL_INTERNAL_ERROR,
        true,
        "foreground tier delivered; readback failed",
    );
    attempt.foreground_authorization = Some(crate::m2::FOREGROUND_AUTH_CLAIMED_TARGET.to_owned());
    attempt
}

fn uia_attempt() -> ActClickTierAttempt {
    click_tier_failed(
        "uia",
        "postdispatch_readback_failed",
        error_codes::TOOL_INTERNAL_ERROR,
        false,
        "uia readback failed",
    )
}

#[test]
fn must_pass_a_reconciled_claimed_foreground_attempt_keeps_claimed_target() {
    let attempts = vec![claimed_foreground_attempt()];
    let reconciled = reconciled_click_attempt(&attempts, &postdispatch_error());
    assert_eq!(reconciled.status, "delivered");
    assert_eq!(reconciled.tier, CLICK_TIER_FOREGROUND);
    assert!(reconciled.required_foreground);
    assert_eq!(
        reconciled.foreground_authorization.as_deref(),
        Some(crate::m2::FOREGROUND_AUTH_CLAIMED_TARGET)
    );
}

#[test]
fn must_fail_a_reconciled_background_attempt_never_inherits_foreground_use() {
    // A claimed foreground attempt elsewhere in the list must not make the
    // reconciled (background) attempt an unstamped foreground attempt.
    let attempts = vec![uia_attempt(), claimed_foreground_attempt()];
    let reconciled = reconciled_click_attempt(&attempts, &postdispatch_error());
    assert_eq!(reconciled.tier, "uia");
    assert!(!reconciled.required_foreground);
    assert!(reconciled.foreground_authorization.is_none());
}

#[test]
fn must_pass_no_prior_attempt_reconciles_as_background_uia() {
    let reconciled = reconciled_click_attempt(&[], &postdispatch_error());
    assert_eq!(reconciled.tier, "uia");
    assert!(!reconciled.required_foreground);
    assert!(reconciled.foreground_authorization.is_none());
}

/// SOURCE-SCAN GUARD: pins that the reconcile call site binds the value of
/// `reconciled_click_attempt(..)` and pushes that very value into the response
/// attempts, so the result can never be discarded or replaced by a hand-built
/// (unstamped) foreground attempt.
#[test]
fn must_fail_the_reconcile_call_site_builds_its_attempt_from_the_reconciled_one() {
    let body = method_body("reconcile_click_postdispatch_error").replace("\r\n", "\n");
    assert!(
        body.contains("let reconciled = reconciled_click_attempt(&tier_attempts, error);"),
        "the reconciled attempt must be built from the attempts and the error"
    );
    assert!(
        body.contains("tier_attempts.push(reconciled);"),
        "the reconciled attempt must be the value pushed into tier_attempts"
    );
    assert!(
        body.contains("let tier = reconciled.tier.clone();"),
        "the reported tier must come from the reconciled attempt"
    );
    assert_eq!(
        body.matches("click_tier_failed(").count(),
        0,
        "the call site must not hand-build an attempt"
    );
    assert_eq!(
        body.matches("ActClickTierAttempt {").count(),
        0,
        "the call site must not hand-build an attempt"
    );
}
