//! #189 / #190 through the verified click router itself.
//!
//! The router body of `act_click_with_verified_router` is
//! `route_verified_click(port, params, started)`, with the one delivery call
//! and the `verify_click_delta` readback injected through `ClickTierPort`.
//! Production passes a port that calls `act_click_with_handle_and_lease` and
//! `verify_click_delta`; these tests pass a scripted port and COUNT how many
//! times delivery really ran.
//!
//! #189: a tier that delivered but showed no delta must not be re-sent through
//! another tier (a Submit button pressed 2-3 times, a checkbox toggled 3 times).
//! #190: the reconciled post-dispatch attempt keeps its claimed-target
//! foreground authorization all the way into the written audit row.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

use super::*;

const SOT: &str = "target_window_ui_or_pixels";
const SESSION: &str = "issue190-router-session";
const TOGGLE_STAYED: &str =
    "TogglePattern.toggle returned for element e1, but ToggleState stayed Off";

type TierResult = Result<ActClickResponse, ErrorData>;

/// Scripted tier port: each tier returns its scripted result once and every
/// call is recorded in order.
#[derive(Default)]
struct ScriptedPort {
    first: Mutex<Option<TierResult>>,
    verify: Mutex<VecDeque<Result<ActClickPostcondition, ErrorData>>>,
    calls: Mutex<Vec<&'static str>>,
}

impl ScriptedPort {
    fn take(&self, slot: &Mutex<Option<TierResult>>, name: &'static str) -> TierResult {
        self.calls.lock().expect("calls").push(name);
        slot.lock().expect("slot").take().unwrap_or_else(|| {
            Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("unscripted {name} tier call"),
            ))
        })
    }

    fn count(&self, name: &str) -> usize {
        let calls = self.calls.lock().expect("calls");
        calls.iter().filter(|call| **call == name).count()
    }
}

impl ClickTierPort for ScriptedPort {
    async fn deliver(&self, _params: ActClickParams) -> TierResult {
        self.take(&self.first, "first")
    }

    async fn verify(&self) -> Result<ActClickPostcondition, ErrorData> {
        self.calls.lock().expect("calls").push("verify");
        self.verify
            .lock()
            .expect("verify")
            .pop_front()
            .unwrap_or_else(|| {
                Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "unscripted verify",
                ))
            })
    }
}

fn element_params() -> ActClickParams {
    serde_json::from_value(json!({ "target": { "element_id": "0x1:0000002a00000001" } }))
        .expect("element click params")
}

fn response(tier: &str, tier_attempts: Vec<ActClickTierAttempt>) -> ActClickResponse {
    ActClickResponse {
        ok: true,
        used_invoke_pattern: tier == "uia",
        backend_used: tier.to_owned(),
        backend_tier_used: tier.to_owned(),
        required_foreground: tier == CLICK_TIER_FOREGROUND,
        tier_attempts,
        postcondition: postcondition_not_requested("act_click", SOT),
        press_hold_ms: 120,
        double_click_window_ms: 500,
        inter_click_delay_ms: 0,
        elapsed_ms: 1,
    }
}

fn no_delta() -> ErrorData {
    source_no_observed_delta_error(
        "act_click",
        SOT,
        250,
        "before".to_owned(),
        "after".to_owned(),
        json!({}),
    )
}

fn observed() -> ActClickPostcondition {
    postcondition_observed_delta(
        "act_click",
        SOT,
        "before".to_owned(),
        "after".to_owned(),
        "target window changed",
    )
}

/// The typed shape the element loop emits for a returned Toggle whose
/// readback stayed (`ClickDispatch { delivered: true }` -> stage "readback").
fn toggle_readback_failed(attempts: Vec<ActClickTierAttempt>) -> ErrorData {
    let mut error = mark_input_delivered(attach_click_tier_attempts(
        mcp_error(error_codes::ACTION_TARGET_INVALID, TOGGLE_STAYED),
        attempts,
    ));
    if let Some(data) = error.data.as_mut().and_then(Value::as_object_mut) {
        data.insert("click_dispatch_stage".to_owned(), json!("readback"));
    }
    error
}

fn foreground_attempt(authorization: Option<&str>) -> ActClickTierAttempt {
    let mut attempt = click_tier_delivered(CLICK_TIER_FOREGROUND, true, "SendInput click");
    attempt.foreground_authorization = authorization.map(str::to_owned);
    attempt
}

async fn route(port: &ScriptedPort) -> TierResult {
    // Precondition: the scripted params really are a background-first element click.
    let params = element_params();
    assert!(matches!(
        params.target,
        crate::m2::ActClickTarget::Element(_)
    ));
    route_verified_click(port, params, Instant::now()).await
}

fn assert_delivered_no_delta(error: &ErrorData) {
    let data = error.data.as_ref().expect("structured error data");
    assert_eq!(
        data["code"],
        error_codes::ACTION_NO_OBSERVED_DELTA,
        "{data}"
    );
    assert_eq!(data["delivered"], true, "{data}");
    assert_eq!(
        data["retry_hint"],
        crate::m2::postcondition::DELIVERED_NO_DELTA_RETRY_HINT,
        "{data}"
    );
}

#[tokio::test]
async fn must_fail_delivered_uia_tier_without_delta_is_delivered_exactly_once() {
    let port = ScriptedPort::default();
    *port.first.lock().expect("first") = Some(Ok(response(
        "uia",
        vec![click_tier_delivered(
            "uia",
            false,
            "InvokePattern.Invoke returned",
        )],
    )));
    port.verify
        .lock()
        .expect("verify")
        .push_back(Err(no_delta()));

    let Err(error) = route(&port).await else {
        panic!("no observed delta must not be reported as verified");
    };

    assert_delivered_no_delta(&error);
    assert_eq!(port.count("first"), 1, "re-delivered");
    assert_eq!(port.count("verify"), 1);
    let attempts = click_tier_attempts_from_error(&error);
    let statuses: Vec<&str> = attempts.iter().map(|a| a.status.as_str()).collect();
    assert_eq!(statuses, ["delivered", "failed"]);
    assert!(attempts.iter().all(|attempt| attempt.tier == "uia"));
}

#[tokio::test]
async fn must_fail_returned_toggle_with_unchanged_target_is_never_toggled_again() {
    let port = ScriptedPort::default();
    *port.first.lock().expect("first") =
        Some(Err(toggle_readback_failed(vec![click_tier_delivered(
            "uia",
            false,
            "TogglePattern.Toggle returned",
        )])));
    port.verify
        .lock()
        .expect("verify")
        .push_back(Err(no_delta()));

    let Err(error) = route(&port).await else {
        panic!("an unchanged target must not reconcile to success");
    };

    let data = error.data.as_ref().expect("structured error data");
    assert_eq!(data["code"], error_codes::ACTION_TARGET_INVALID, "{data}");
    assert_eq!(data["delivered"], true, "{data}");
    assert_eq!(port.count("first"), 1, "checkbox toggled a second time");
}

#[tokio::test]
async fn must_pass_delivered_and_verified_click_is_sent_once_and_succeeds() {
    let port = ScriptedPort::default();
    *port.first.lock().expect("first") = Some(Ok(response(
        "uia",
        vec![click_tier_delivered(
            "uia",
            false,
            "InvokePattern.Invoke returned",
        )],
    )));
    port.verify
        .lock()
        .expect("verify")
        .push_back(Ok(observed()));

    let response = route(&port).await.expect("verified click");

    assert_eq!(response.postcondition.status, "observed_delta");
    assert_eq!(response.backend_tier_used, "uia");
    assert_eq!(port.count("first"), 1);
}

fn last_action_row(service: &SynapseService) -> Value {
    let runtime = service.reflex_runtime().expect("runtime");
    let rows = runtime
        .lock()
        .expect("lock")
        .storage_cf_tail_rows(synapse_storage::cf::CF_ACTION_LOG, 1)
        .expect("tail rows");
    let (_key, bytes) = rows.into_iter().next().expect("one audit row");
    synapse_storage::decode_json::<Value>(&bytes).expect("decode audit row")
}

async fn reconciled_row(first_attempt: ActClickTierAttempt) -> (ActClickResponse, Value) {
    let port = ScriptedPort::default();
    *port.first.lock().expect("first") = Some(Err(toggle_readback_failed(vec![first_attempt])));
    port.verify
        .lock()
        .expect("verify")
        .push_back(Ok(observed()));
    let response = route(&port).await.expect("target-window delta reconciles");
    assert_eq!(port.count("first"), 1, "reconciliation never re-delivers");
    let dir = tempfile::tempdir().expect("tmp");
    let service = crate::test_support::service_in(dir.path()).expect("service");
    service
        .audit_action_result_for_session("act_click", &Ok(response.clone()), SESSION)
        .expect("write audit row");
    (response, last_action_row(&service))
}

#[tokio::test]
async fn must_pass_reconciled_claimed_foreground_click_is_audited_as_claimed_route() {
    let (response, row) = reconciled_row(foreground_attempt(Some("claimed_target"))).await;

    let last = response.tier_attempts.last().expect("reconciled attempt");
    assert_eq!(last.tier, CLICK_TIER_FOREGROUND);
    assert_eq!(last.status, "delivered");
    assert!(last.required_foreground);
    assert_eq!(
        last.foreground_authorization.as_deref(),
        Some("claimed_target")
    );
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], true, "row: {row}");
    assert_eq!(tier["claimed_target_route"], true, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], false, "row: {row}");
}

#[tokio::test]
async fn must_fail_reconciled_unsanctioned_foreground_click_stays_a_violation() {
    let (response, row) = reconciled_row(foreground_attempt(None)).await;

    let last = response.tier_attempts.last().expect("reconciled attempt");
    assert!(last.required_foreground);
    assert_eq!(last.foreground_authorization, None);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], true, "row: {row}");
    assert_eq!(tier["claimed_target_route"], false, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], true, "row: {row}");
    assert_eq!(tier["allowed"], false, "row: {row}");
}

#[tokio::test]
async fn must_pass_reconciled_background_click_never_inherits_foreground_use() {
    let (response, row) = reconciled_row(click_tier_delivered(
        "uia",
        false,
        "TogglePattern.Toggle returned",
    ))
    .await;

    assert!(!response.required_foreground);
    assert_eq!(response.backend_tier_used, "uia");
    let last = response.tier_attempts.last().expect("reconciled attempt");
    assert!(!last.required_foreground);
    assert_eq!(last.foreground_authorization, None);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], false, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], false, "row: {row}");
}
