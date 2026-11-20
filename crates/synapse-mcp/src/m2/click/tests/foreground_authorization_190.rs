//! #190: every foreground (SendInput) click attempt records how the tier was
//! sanctioned, on success and on error rows, so the audit can tell the #187
//! claimed-window route from a policy violation.

use std::sync::Arc;

use rmcp::model::{ErrorCode, ErrorData};
use serde_json::json;
use synapse_action::{ActionBackend, ActionEmitter, RecordingBackend};
use synapse_core::error_codes;
use tokio_util::sync::CancellationToken;

use super::{
    CLICK_TIER_FOREGROUND, CLICK_TIER_POSTMESSAGE, ClaimFacts, FOREGROUND_AUTH_CLAIMED_TARGET,
    FOREGROUND_AUTH_PROFILE, ForegroundClickPolicy, act_click_with_handle_and_lease,
    authorize_foreground_click, click_error_data_code,
    schema::{
        ActClickParams, ActClickPointTarget, ActClickPostcondition, ActClickResponse,
        ActClickTarget, ActClickTierAttempt, default_click_backend, default_click_button,
        default_click_count, default_click_duration_ms, default_click_hold_ms,
        default_click_velocity_profile, default_coordinate_fallback_on_unsupported,
        default_use_invoke_pattern, default_verify_timeout_ms,
    },
    stamp_foreground_authorization,
};

#[test]
fn must_pass_an_error_row_foreground_attempt_is_stamped_claimed_target() {
    let error = ErrorData::new(
        ErrorCode(-32099),
        "click delivered but unverified",
        Some(json!({
            "tier_attempts": [
                { "tier": CLICK_TIER_POSTMESSAGE, "required_foreground": false },
                { "tier": CLICK_TIER_FOREGROUND, "required_foreground": true, "status": "delivered" },
            ],
        })),
    );
    let Err(error) =
        stamp_foreground_authorization(Err(error), Some(FOREGROUND_AUTH_CLAIMED_TARGET))
    else {
        panic!("an error stays an error");
    };
    let attempts = error.data.unwrap_or_default()["tier_attempts"].clone();
    assert_eq!(
        attempts[1]["foreground_authorization"],
        FOREGROUND_AUTH_CLAIMED_TARGET
    );
    assert!(attempts[0].get("foreground_authorization").is_none());
}

#[tokio::test]
async fn must_fail_a_refused_foreground_tier_is_never_stamped() {
    let _serial = crate::test_support::lease_serial("refused_foreground_tier_190");
    let denial = authorize_foreground_click(
        false,
        &ClaimFacts {
            target_root: None,
            claim_held: false,
            session_target_matches: false,
        },
    )
    .expect_err("a normal profile without a claim is denied");
    let cancel = CancellationToken::new();
    let recording = Arc::new(RecordingBackend::new());
    let backend: Arc<dyn ActionBackend> = recording.clone();
    let (handle, _snapshot_handle, join) =
        ActionEmitter::spawn_with_backend(cancel.clone(), backend);

    let result = act_click_with_handle_and_lease(
        handle,
        None,
        point_click_params(),
        ForegroundClickPolicy::authorized(Some("refused-click-190".to_owned()), Err(denial)),
    )
    .await;
    let Err(error) = result else {
        panic!("a denied foreground policy must refuse the SendInput tier");
    };

    assert_eq!(
        click_error_data_code(&error),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED)
    );
    let attempts = error.data.unwrap_or_default()["tier_attempts"].clone();
    assert_eq!(attempts[0]["tier"], CLICK_TIER_FOREGROUND);
    assert_eq!(attempts[0]["status"], "failed");
    assert!(
        attempts[0].get("foreground_authorization").is_none(),
        "a refused tier must not claim an authorization: {attempts}"
    );
    assert!(recording.events().is_empty(), "{:?}", recording.events());
    cancel.cancel();
    let _ = join.await;
}

fn point_click_params() -> ActClickParams {
    ActClickParams {
        target: ActClickTarget::Point(ActClickPointTarget { x: 12, y: 34 }),
        button: default_click_button(),
        clicks: default_click_count(),
        modifiers: Vec::new(),
        velocity_profile: default_click_velocity_profile(),
        duration_ms: default_click_duration_ms(),
        hold_ms: default_click_hold_ms(),
        backend: default_click_backend(),
        use_invoke_pattern: default_use_invoke_pattern(),
        coordinate_fallback_on_unsupported: default_coordinate_fallback_on_unsupported(),
        verify_delta: false,
        verify_target_window_hwnd: None,
        verify_timeout_ms: default_verify_timeout_ms(),
        auto_wait: false,
        auto_wait_timeout_ms: crate::m2::default_auto_wait_timeout_ms(),
        deprecated_curve_alias_used: false,
        coordinate_first: false,
    }
}

fn tier_attempt(tier: &str, required_foreground: bool) -> ActClickTierAttempt {
    ActClickTierAttempt {
        tier: tier.to_owned(),
        status: "failed".to_owned(),
        reason_code: None,
        error_code: None,
        detail: None,
        required_foreground,
        foreground_authorization: None,
    }
}

#[test]
fn must_pass_success_row_foreground_attempt_is_stamped_profile() {
    let mut delivered = tier_attempt(CLICK_TIER_FOREGROUND, true);
    delivered.status = "delivered".to_owned();
    let response = ActClickResponse {
        ok: true,
        used_invoke_pattern: false,
        backend_used: "software".to_owned(),
        backend_tier_used: CLICK_TIER_FOREGROUND.to_owned(),
        required_foreground: true,
        tier_attempts: vec![tier_attempt(CLICK_TIER_POSTMESSAGE, false), delivered],
        postcondition: ActClickPostcondition {
            status: "not_requested".to_owned(),
            observed_delta: None,
            source_of_truth: None,
            before_signature: None,
            after_signature: None,
            detail: None,
            related_window_observed: None,
        },
        press_hold_ms: 120,
        double_click_window_ms: 500,
        inter_click_delay_ms: 0,
        elapsed_ms: 1,
    };
    let Ok(response) = stamp_foreground_authorization(Ok(response), Some(FOREGROUND_AUTH_PROFILE))
    else {
        panic!("a success stays a success");
    };
    assert_eq!(
        response.tier_attempts[1]
            .foreground_authorization
            .as_deref(),
        Some(FOREGROUND_AUTH_PROFILE)
    );
    assert_eq!(response.tier_attempts[0].foreground_authorization, None);
}
