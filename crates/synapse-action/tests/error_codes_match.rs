use synapse_action::ActionError;
use synapse_core::{ElementId, error_codes};

const DETAIL: &str = "synthetic detail";
const RETRY_AFTER_MS: u64 = 1;

/// Number of `ActionError` variants. `case_label` below matches exhaustively
/// with no wildcard, so adding a variant breaks the compile there; bump this
/// count and add the variant to `action_error_cases` in the same change.
const VARIANT_COUNT: usize = 24;

/// Exhaustive (no wildcard): the edge label plus the core literal each variant
/// must report. A new `ActionError` variant fails to compile here.
const fn case_label(error: &ActionError) -> (&'static str, &'static str) {
    match error {
        ActionError::ClickDispatch { source, .. } => ("click_dispatch", source.code()),
        ActionError::ClickNotDispatched { source, .. } => ("click_not_dispatched", source.code()),
        ActionError::QueueFull { .. } => ("queue_full", error_codes::ACTION_QUEUE_FULL),
        ActionError::RateLimited { .. } => ("rate_limited", error_codes::ACTION_RATE_LIMITED),
        ActionError::ForegroundLeaseBusy { .. } => (
            "foreground_lease_busy",
            error_codes::ACTION_FOREGROUND_LEASE_BUSY,
        ),
        ActionError::BackendUnavailable { .. } => (
            "backend_unavailable",
            error_codes::ACTION_BACKEND_UNAVAILABLE,
        ),
        ActionError::InputTargetRequired { .. } => (
            "input_target_required",
            error_codes::ACTION_INPUT_TARGET_REQUIRED,
        ),
        ActionError::InputTargetNotReady { .. } => (
            "input_target_not_ready",
            error_codes::ACTION_INPUT_TARGET_NOT_READY,
        ),
        ActionError::ForegroundActivationRefused { .. } => (
            "foreground_activation_refused",
            error_codes::FOREGROUND_ACTIVATION_REFUSED,
        ),
        ActionError::TargetInvalid { .. } => ("target_invalid", error_codes::ACTION_TARGET_INVALID),
        ActionError::WindowNotFound { .. } => {
            ("window_not_found", error_codes::TARGET_WINDOW_NOT_FOUND)
        }
        ActionError::HoldExceededMax { .. } => {
            ("hold_exceeded_max", error_codes::ACTION_HOLD_EXCEEDED_MAX)
        }
        ActionError::VigemNotInstalled { .. } => (
            "vigem_not_installed",
            error_codes::ACTION_VIGEM_NOT_INSTALLED,
        ),
        ActionError::VigemPluginFailed { .. } => (
            "vigem_plugin_failed",
            error_codes::ACTION_VIGEM_PLUGIN_FAILED,
        ),
        ActionError::ElementNotResolved { .. } => (
            "element_not_resolved",
            error_codes::ACTION_ELEMENT_NOT_RESOLVED,
        ),
        ActionError::ElementPatternUnsupported { .. } => (
            "element_pattern_unsupported",
            error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED,
        ),
        ActionError::TransientElementExpired { .. } => (
            "transient_element_expired",
            error_codes::TRANSIENT_ELEMENT_EXPIRED,
        ),
        ActionError::ForegroundLost { .. } => {
            ("foreground_lost", error_codes::ACTION_FOREGROUND_LOST)
        }
        ActionError::UnsupportedKey { .. } => {
            ("unsupported_key", error_codes::ACTION_UNSUPPORTED_KEY)
        }
        ActionError::NoObservedDelta { .. } => {
            ("no_observed_delta", error_codes::ACTION_NO_OBSERVED_DELTA)
        }
        ActionError::DragDistanceExceedsLimit { .. } => (
            "drag_distance_exceeds_limit",
            error_codes::ACTION_DRAG_DISTANCE_EXCEEDS_LIMIT,
        ),
        ActionError::StuckKeyAutoReleased { .. } => (
            "stuck_key_auto_released",
            error_codes::STUCK_KEY_AUTO_RELEASED,
        ),
        ActionError::SafetyReleaseAllFired { .. } => (
            "safety_release_all_fired",
            error_codes::SAFETY_RELEASE_ALL_FIRED,
        ),
        ActionError::SafetyOperatorHotkeyFired { .. } => (
            "safety_operator_hotkey_fired",
            error_codes::SAFETY_OPERATOR_HOTKEY_FIRED,
        ),
    }
}

#[test]
fn must_pass_action_error_codes_match_core_literals() {
    let cases = action_error_cases();
    assert_eq!(cases.len(), VARIANT_COUNT, "every variant needs one case");
    for (variant, error, expected) in cases {
        let before = format!("{error:?}");
        let after = error.code();
        assert_eq!(after, expected, "variant={variant}");
        assert_eq!(error.detail(), DETAIL, "variant={variant}");
        println!("readback=error_codes_match edge={variant} before={before} after={after}");
    }
}

#[test]
fn must_fail_distinct_source_errors_never_share_a_code() {
    let cases = action_error_cases();
    assert_eq!(cases.len(), VARIANT_COUNT, "every variant needs one case");
    for (index, (variant, error, _expected)) in cases.iter().enumerate() {
        assert!(
            !error.code().is_empty(),
            "variant {variant} has an empty code"
        );
        for (other_variant, other, _other_expected) in cases.iter().skip(index + 1) {
            assert_ne!(
                variant, other_variant,
                "variant {variant} is listed twice; another variant is missing"
            );
            // The click wrappers add delivery evidence without changing the source code.
            let wrapper = |error: &ActionError| {
                matches!(
                    error,
                    ActionError::ClickDispatch { .. } | ActionError::ClickNotDispatched { .. }
                )
            };
            if wrapper(error) || wrapper(other) {
                continue;
            }
            assert_ne!(
                error.code(),
                other.code(),
                "variants {variant} and {other_variant} collide on one error code"
            );
        }
    }
}

#[test]
fn must_fail_with_detail_cannot_change_code_or_keep_old_detail() {
    for (variant, error, expected) in action_error_cases() {
        let rewritten = error.clone().with_detail("");
        assert_eq!(rewritten.code(), expected, "variant={variant}");
        assert_eq!(rewritten.detail(), "", "variant={variant}");
        assert_eq!(
            rewritten.retry_after_ms(),
            error.retry_after_ms(),
            "variant={variant}"
        );
        // Every non-detail field survives the rewrite.
        assert_eq!(
            rewritten.with_detail(DETAIL),
            error,
            "variant={variant} lost a field"
        );
    }
}

#[test]
fn must_fail_non_retryable_variants_never_report_retry_after() {
    for (variant, error, _expected) in action_error_cases() {
        let retryable = matches!(
            error,
            ActionError::RateLimited { .. } | ActionError::ForegroundLeaseBusy { .. }
        );
        let expected_retry = retryable.then_some(RETRY_AFTER_MS);
        assert_eq!(error.retry_after_ms(), expected_retry, "variant={variant}");
    }
}

fn element_id() -> ElementId {
    match ElementId::parse("0x1234:0000002a00000001") {
        Ok(element_id) => element_id,
        Err(error) => panic!("synthetic element id must parse: {error}"),
    }
}

fn action_error_cases() -> Vec<(&'static str, ActionError, &'static str)> {
    let detail = || DETAIL.to_owned();
    let errors = vec![
        ActionError::ClickDispatch {
            source: Box::new(ActionError::TargetInvalid { detail: detail() }),
            delivered: false,
        },
        ActionError::ClickNotDispatched {
            source: Box::new(ActionError::BackendUnavailable { detail: detail() }),
            tier_unavailable: true,
        },
        ActionError::QueueFull { detail: detail() },
        ActionError::RateLimited {
            detail: detail(),
            retry_after_ms: RETRY_AFTER_MS,
        },
        ActionError::ForegroundLeaseBusy {
            detail: detail(),
            holder_session_id: Some("holder".to_owned()),
            requesting_session_id: "requester".to_owned(),
            retry_after_ms: RETRY_AFTER_MS,
        },
        ActionError::BackendUnavailable { detail: detail() },
        ActionError::InputTargetRequired { detail: detail() },
        ActionError::InputTargetNotReady { detail: detail() },
        ActionError::ForegroundActivationRefused { detail: detail() },
        ActionError::TargetInvalid { detail: detail() },
        ActionError::WindowNotFound { detail: detail() },
        ActionError::HoldExceededMax { detail: detail() },
        ActionError::VigemNotInstalled { detail: detail() },
        ActionError::VigemPluginFailed { detail: detail() },
        ActionError::ElementNotResolved { detail: detail() },
        ActionError::ElementPatternUnsupported {
            element_id: element_id(),
            detail: detail(),
        },
        ActionError::TransientElementExpired {
            element_id: element_id(),
            detail: detail(),
        },
        ActionError::ForegroundLost { detail: detail() },
        ActionError::UnsupportedKey { detail: detail() },
        ActionError::NoObservedDelta { detail: detail() },
        ActionError::DragDistanceExceedsLimit { detail: detail() },
        ActionError::StuckKeyAutoReleased { detail: detail() },
        ActionError::SafetyReleaseAllFired { detail: detail() },
        ActionError::SafetyOperatorHotkeyFired { detail: detail() },
    ];
    errors
        .into_iter()
        .map(|error| {
            let (label, code) = case_label(&error);
            (label, error, code)
        })
        .collect()
}
