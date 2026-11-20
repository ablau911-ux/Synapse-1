//! #189: exercise the production router with an injected delivery/readback port.
//! The router owns no escalation: every tier change happens inside the
//! delivery call, and a returned error is terminal here.

use std::sync::{Mutex, MutexGuard};

use super::*;

#[path = "click_router_dispatch_189.rs"]
mod dispatch_189;

#[path = "click_classification_189.rs"]
mod click_classification_189;

type TierResult = Result<ActClickResponse, ErrorData>;

struct Script {
    first: Option<TierResult>,
    verification: Option<Result<ActClickPostcondition, ErrorData>>,
    calls: Vec<&'static str>,
}

struct Port(Mutex<Script>);

impl Port {
    fn new(first: ErrorData, verification: Result<ActClickPostcondition, ErrorData>) -> Self {
        Self::with_first(Err(first), verification)
    }

    fn with_first(
        first: TierResult,
        verification: Result<ActClickPostcondition, ErrorData>,
    ) -> Self {
        Self(Mutex::new(Script {
            first: Some(first),
            verification: Some(verification),
            calls: Vec::new(),
        }))
    }

    fn state(&self) -> MutexGuard<'_, Script> {
        self.0
            .lock()
            .unwrap_or_else(|error| panic!("script lock: {error}"))
    }

    fn calls(&self) -> Vec<&'static str> {
        self.state().calls.clone()
    }
}

impl ClickTierPort for Port {
    async fn deliver(&self, _params: ActClickParams) -> TierResult {
        let mut script = self.state();
        script.calls.push("first");
        script
            .first
            .take()
            .unwrap_or_else(|| panic!("delivery called twice"))
    }

    async fn verify(&self) -> Result<ActClickPostcondition, ErrorData> {
        let mut script = self.state();
        script.calls.push("verify");
        script
            .verification
            .take()
            .unwrap_or_else(|| Err(no_delta()))
    }
}

fn params() -> ActClickParams {
    serde_json::from_value(json!({
        "target": { "element_id": "0x1:0000002a00000001" },
        "verify_delta": true
    }))
    .unwrap_or_else(|error| panic!("element params: {error}"))
}

fn delivered(tier: &str) -> ActClickResponse {
    ActClickResponse {
        ok: true,
        used_invoke_pattern: false,
        backend_used: tier.to_owned(),
        backend_tier_used: tier.to_owned(),
        required_foreground: tier == CLICK_TIER_FOREGROUND,
        tier_attempts: vec![click_tier_delivered(
            tier,
            tier == CLICK_TIER_FOREGROUND,
            "delivered",
        )],
        postcondition: postcondition_not_requested("act_click", "target_window_ui_or_pixels"),
        press_hold_ms: 120,
        double_click_window_ms: 500,
        inter_click_delay_ms: 0,
        elapsed_ms: 1,
    }
}

fn observed() -> ActClickPostcondition {
    postcondition_observed_delta(
        "act_click",
        "target_window_ui_or_pixels",
        "before".to_owned(),
        "after".to_owned(),
        "target changed",
    )
}

fn no_delta() -> ErrorData {
    source_no_observed_delta_error(
        "act_click",
        "target_window_ui_or_pixels",
        250,
        "same".to_owned(),
        "same".to_owned(),
        json!({}),
    )
}

/// The element loop's typed shape for a failure before any dispatch.
fn undelivered() -> ErrorData {
    attach_click_tier_attempts(
        crate::m2::dispatch_189_test_support::before_dispatch(crate::m2::action_error_to_mcp(
            &synapse_action::ActionError::BackendUnavailable {
                detail: "UIA tier could not run".to_owned(),
            },
        )),
        vec![click_tier_failed(
            "uia",
            "backend_unavailable",
            error_codes::ACTION_BACKEND_UNAVAILABLE,
            false,
            "no input delivered",
        )],
    )
}

async fn route(port: &Port, params: ActClickParams) -> TierResult {
    route_verified_click(port, params, Instant::now()).await
}

/// Item 8: the router has no Err escalation ladder. Even a proven-undelivered
/// first-tier error returns unchanged after exactly one delivery call.
#[tokio::test]
async fn must_fail_undelivered_first_tier_error_is_terminal_at_the_router() {
    let first = undelivered();
    let expected = first.clone();
    let port = Port::new(first, Ok(observed()));
    let error = route(&port, params())
        .await
        .expect_err("router never re-delivers");
    assert_eq!(
        port.calls(),
        ["first"],
        "no second delivery, no observation"
    );
    assert_eq!(
        error.data, expected.data,
        "proven non-delivery is preserved"
    );
    crate::server::background_router::dispatch_status_189_tests::assert_not_delivered(
        &error,
        "failed_before_delivery",
    );
}

/// R5 router default: an error with no typed evidence is unknown delivery,
/// whatever its code (HEAD let ACTION_TARGET_INVALID pass as undelivered).
#[tokio::test]
async fn must_fail_untyped_first_tier_error_defaults_to_possible_delivery() {
    for code in [
        error_codes::ACTION_TARGET_INVALID,
        error_codes::ACTION_BACKEND_UNAVAILABLE,
        error_codes::TOOL_INTERNAL_ERROR,
    ] {
        let port = Port::new(mcp_error(code, "no dispatch evidence"), Ok(observed()));
        let error = route(&port, params()).await.expect_err("unknown must stop");
        assert_eq!(port.calls(), ["first"], "{code}");
        crate::server::background_router::dispatch_status_189_tests::assert_verify_needed(&error);
        assert_eq!(error.data.as_ref().unwrap()["possibly_delivered"], true);
    }
}

#[tokio::test]
async fn must_fail_refused_coordinate_fallback_never_calls_another_tier() {
    let source = synapse_action::ActionError::ElementPatternUnsupported {
        element_id: synapse_core::ElementId::parse("0x1:0000002a00000001")
            .unwrap_or_else(|error| panic!("element id: {error:?}")),
        detail: "static pane exposes no click control pattern".to_owned(),
    };
    let refusal = crate::m2::click_metadata_refusal_error(&source);
    let data = refusal
        .data
        .as_ref()
        .unwrap_or_else(|| panic!("refusal data"));
    assert_eq!(
        data["code"],
        error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED
    );
    assert_eq!(data["router_escalation_allowed"], false);
    assert_eq!(
        data["router_stop_reason"],
        "coordinate_fallback_metadata_denied"
    );
    assert_eq!(click_tier_attempts_from_error(&refusal).len(), 1);
    assert!(!click_tier_attempts_delivered(
        &click_tier_attempts_from_error(&refusal)
    ));
    let expected = refusal.clone();
    let port = Port::new(refusal, Ok(observed()));
    let result = route(&port, params()).await;
    assert_eq!(
        port.calls(),
        ["first"],
        "metadata refusal must receive no PostMessage, SendInput or verification"
    );
    let Err(error) = result else {
        panic!("coordinate refusal must be returned")
    };
    assert_eq!(error.code, expected.code);
    assert_eq!(error.message, expected.message);
    assert_eq!(
        error.data, expected.data,
        "preserve original refusal and failed UIA attempt"
    );
}

/// P5: an in-tier PostMessage that delivered but showed no delta is final.
#[tokio::test]
async fn must_fail_delivered_postmessage_without_delta_never_reaches_sendinput() {
    let mut response = delivered(CLICK_TIER_POSTMESSAGE);
    response.tier_attempts.insert(
        0,
        click_tier_failed(
            "uia",
            "pattern_unsupported",
            error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED,
            false,
            "no UIA pattern",
        ),
    );
    let port = Port::with_first(Ok(response), Err(no_delta()));
    let error = route(&port, params())
        .await
        .expect_err("unchanged target must remain unverified");
    assert_eq!(port.calls(), ["first", "verify"]);
    assert_eq!(
        click_error_data_code(&error),
        Some(error_codes::ACTION_NO_OBSERVED_DELTA)
    );
    assert_eq!(error.data.as_ref().unwrap()["delivered"], true);
    let attempts = click_tier_attempts_from_error(&error);
    assert_eq!(
        attempts.iter().filter(|a| a.status == "delivered").count(),
        1
    );
    assert!(attempts.iter().all(|a| a.tier != CLICK_TIER_FOREGROUND));
}

/// P6: a delivered PostMessage with an observed delta is verified.
#[tokio::test]
async fn must_pass_delivered_postmessage_with_delta_is_verified_once() {
    let port = Port::with_first(Ok(delivered(CLICK_TIER_POSTMESSAGE)), Ok(observed()));
    let response = route(&port, params()).await.expect("verified");
    assert_eq!(port.calls(), ["first", "verify"]);
    assert_eq!(response.backend_tier_used, CLICK_TIER_POSTMESSAGE);
    assert_eq!(response.postcondition.status, "observed_delta");
}

/// U21: a closed target window counts as verified only on the element route.
#[test]
fn must_pass_dialog_dismissal_is_verified_only_for_semantic_routes() {
    let closed = || mcp_error(error_codes::TARGET_WINDOW_NOT_FOUND, "dialog dismissed");
    let response =
        verified_click_router::finish_verified_click(delivered("uia"), Err(closed()), true)
            .expect("element route: dismissal is the effect");
    assert_eq!(response.postcondition.status, "observed_delta");
    assert_eq!(
        response.postcondition.after_signature.as_deref(),
        Some("target_window_closed")
    );
    let error = verified_click_router::finish_verified_click(
        delivered(CLICK_TIER_POSTMESSAGE),
        Err(closed()),
        false,
    )
    .expect_err("coordinate route: no observed effect");
    assert_eq!(error.data.as_ref().unwrap()["delivered"], true);
}

/// U22: a verify-surface failure after delivery keeps delivered_unverified.
#[tokio::test]
async fn must_fail_verify_surface_failure_after_delivery_keeps_delivery() {
    let port = Port::with_first(
        Ok(delivered("uia")),
        Err(mcp_error(
            error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE,
            "capture surface unavailable",
        )),
    );
    let error = route(&port, params()).await.expect_err("unverified");
    assert_eq!(port.calls(), ["first", "verify"]);
    assert_eq!(error.data.as_ref().unwrap()["delivered"], true);
    crate::server::background_router::dispatch_status_189_tests::assert_verify_needed(&error);
}
