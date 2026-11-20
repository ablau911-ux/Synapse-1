//! Regression tests for #53: act foreground escalation reasons must stay within
//! the tool-profile reason bound so the profile restore after the action can
//! never be refused for length.

use super::*;

#[test]
fn must_fail_bounded_profile_reason_never_exceeds_the_profile_reason_limit() {
    let prefix = "restore after act_foreground";
    for detail_len in [0, 1, 990, 994, 995, 1000, MAX_PROFILE_REASON_CHARS] {
        let detail = "V".repeat(detail_len);
        let reason = bounded_profile_reason(prefix, &detail);
        assert!(
            reason.chars().count() <= MAX_PROFILE_REASON_CHARS,
            "detail_len={detail_len} produced {} chars",
            reason.chars().count()
        );
        assert!(reason.starts_with(prefix));
        assert!(
            normalize_reason(Some(&reason)).is_ok(),
            "detail_len={detail_len} must pass tool_profile_set validation"
        );
    }
}

#[test]
fn must_pass_bounded_profile_reason_keeps_short_details_verbatim() {
    assert_eq!(
        bounded_profile_reason("restore after act_foreground", "  copy cell  "),
        "restore after act_foreground: copy cell"
    );
    assert_eq!(
        bounded_profile_reason("restore after act_foreground", "   "),
        "restore after act_foreground"
    );
}

#[test]
fn must_fail_bounded_profile_reason_truncates_on_char_boundaries() {
    let detail = "\u{05e9}".repeat(MAX_PROFILE_REASON_CHARS);
    let reason = bounded_profile_reason("restore after act_foreground", &detail);
    assert_eq!(reason.chars().count(), MAX_PROFILE_REASON_CHARS);
}

#[test]
fn must_fail_normalize_reason_rejects_over_limit_reasons() {
    let at_limit = "R".repeat(MAX_PROFILE_REASON_CHARS);
    let over_limit = "R".repeat(MAX_PROFILE_REASON_CHARS + 1);
    assert!(normalize_reason(Some(&at_limit)).is_ok());
    let error =
        normalize_reason(Some(&over_limit)).expect_err("a reason over the limit must be refused");
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
    assert!(error.message.contains("at most"), "{error:?}");
    assert!(matches!(normalize_reason(Some("   ")), Ok(None)));
}

#[test]
fn must_pass_unwind_confirm_flag_matches_the_profile_set_policy() {
    // act_foreground's unwind derives confirm_break_glass from
    // requires_explicit_confirm; it must agree with the policy that refuses
    // advanced profiles without confirmation, or a restore is silently refused.
    let lease_proof = ToolProfileLeaseProof {
        required: false,
        held: false,
        owner_session_id: None,
        caller_is_owner: false,
        expires_in_ms: None,
    };
    for profile in [
        ToolProfileKind::NormalAgent,
        ToolProfileKind::BrowserControl,
        ToolProfileKind::BrowserDebugger,
        ToolProfileKind::BreakGlass,
        ToolProfileKind::FullCapability,
    ] {
        let unconfirmed =
            validate_profile_set_policy("unwind-test", profile, Some("why"), false, &lease_proof);
        assert_eq!(
            unconfirmed.is_err(),
            profile.requires_explicit_confirm(),
            "profile={}",
            profile.as_str()
        );
    }
}
