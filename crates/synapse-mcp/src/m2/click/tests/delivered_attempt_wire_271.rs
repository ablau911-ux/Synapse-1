//! #271 (from #189 C6/B8): the frozen wire form of a delivered click whose
//! reply failed is `data.delivered = true` AND a `delivered` tier attempt for
//! the delivering tier. C6 is a CDP `mouseReleased` failure after a returned
//! `mousePressed`; B8 is an extension error after DOM events were dispatched.
//! Each case runs through every production composition of the lane:
//! `remote_click_with` around the lane's own error mapping.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::chrome_debugger_bridge::{
    BridgeDispatchStage, dispatch_stage_189_tests::response_error,
};
use crate::server::dispatch_status_189_tests::assert_verify_needed;
use serde_json::json;

fn released_after_pressed(delivered: bool) -> synapse_a11y::A11yError {
    synapse_a11y::A11yError::ClickDispatch {
        source: Box::new(synapse_a11y::A11yError::CdpAxtreeFailed {
            detail: "CDP input dispatch failed: synthetic mouseReleased reply loss".to_owned(),
        }),
        delivered,
    }
}

/// The error a lane returns through `remote_click_with`.
async fn through_remote_click(error: ErrorData) -> ErrorData {
    remote_click_with(async { Err::<(), ErrorData>(error) })
        .await
        .expect_err("the lane failed")
}

fn delivered_attempts(error: &ErrorData) -> Vec<ActClickTierAttempt> {
    click_attempts(error)
        .into_iter()
        .filter(|attempt| attempt.status == "delivered")
        .collect()
}

fn assert_delivered_wire(error: &ErrorData, lane: &str) {
    assert_verify_needed(error);
    let data = error.data.as_ref().expect("structured error data");
    assert_eq!(data["delivered"], true, "{lane}: {error:?}");
    assert_ne!(data["dispatch_not_started"], true, "{lane}: {error:?}");
    assert_eq!(
        data["router_escalation_allowed"], false,
        "{lane}: {error:?}"
    );
    let delivered = delivered_attempts(error);
    assert_eq!(
        delivered.len(),
        1,
        "{lane}: the frozen wire form needs exactly one delivered tier attempt: {error:?}"
    );
    assert_eq!(delivered[0].tier, CLICK_TIER_CDP, "{lane}: delivering tier");
}

fn assert_unknown_delivery_has_no_delivered_attempt(error: &ErrorData, lane: &str) {
    assert_verify_needed(error);
    let data = error.data.as_ref().expect("structured error data");
    assert_eq!(data["possibly_delivered"], true, "{lane}: {error:?}");
    assert_ne!(data["delivered"], true, "{lane}: {error:?}");
    assert!(
        delivered_attempts(error).is_empty(),
        "{lane}: unknown delivery must not claim a delivered tier attempt: {error:?}"
    );
}

/// `execute_cdp_click`: its own mapping attaches the failed CDP attempt.
fn execute_cdp_click_mapping(source: &synapse_a11y::A11yError) -> ErrorData {
    let error = remote_cdp_click_failure(source, mcp_error(source.code(), source.to_string()));
    attach_click_tier_attempts(
        error,
        vec![click_tier_failed(
            CLICK_TIER_CDP,
            click_reason_for_error_code(source.code()),
            source.code(),
            false,
            source.to_string(),
        )],
    )
}

/// `target_act_raw_cdp_coordinate_click` / `target_act_raw_cdp_click_dispatch`:
/// no attempt is attached before `remote_click_with` sees the error.
fn target_act_raw_cdp_mapping(source: &synapse_a11y::A11yError) -> ErrorData {
    remote_cdp_click_failure(source, mcp_error(source.code(), source.to_string()))
}

#[tokio::test]
async fn must_fail_cdp_release_failure_after_press_carries_a_delivered_tier_attempt() {
    let source = released_after_pressed(true);
    assert_delivered_wire(
        &through_remote_click(execute_cdp_click_mapping(&source)).await,
        "execute_cdp_click",
    );
    assert_delivered_wire(
        &through_remote_click(target_act_raw_cdp_mapping(&source)).await,
        "target_act_raw_cdp",
    );
}

#[tokio::test]
async fn must_pass_cdp_press_failure_stays_unknown_without_a_delivered_attempt() {
    // mousePressed itself failed: delivery is unknown, never claimed.
    let source = released_after_pressed(false);
    assert_unknown_delivery_has_no_delivered_attempt(
        &through_remote_click(execute_cdp_click_mapping(&source)).await,
        "execute_cdp_click",
    );
    assert_unknown_delivery_has_no_delivered_attempt(
        &through_remote_click(target_act_raw_cdp_mapping(&source)).await,
        "target_act_raw_cdp",
    );
}

fn extension_error(
    stage: Option<&str>,
) -> crate::chrome_debugger_bridge::ChromeDebuggerBridgeError {
    let mut payload = json!({
        "code": synapse_core::error_codes::CHROME_DOM_ACTION_UNSUPPORTED,
        "detail": "restoreWindowOpen threw after the click events",
    });
    if let Some(stage) = stage {
        payload["dispatch_stage"] = json!(stage);
    }
    response_error(payload)
}

/// `execute_cdp_click` bridge branch: stamp the producer stage, attach the CDP attempt.
fn execute_cdp_click_bridge_mapping(
    error: &crate::chrome_debugger_bridge::ChromeDebuggerBridgeError,
) -> ErrorData {
    let detail = format!("Chrome debugger extension click failed: {}", error.detail());
    attach_click_tier_attempts(
        stamp_bridge_dispatch_stage(
            mcp_error(error.code(), detail.clone()),
            error.dispatch_stage(),
        ),
        vec![click_tier_failed(
            CLICK_TIER_CDP,
            click_reason_for_error_code(error.code()),
            error.code(),
            false,
            detail,
        )],
    )
}

#[tokio::test]
async fn must_fail_extension_error_after_dispatch_carries_a_delivered_tier_attempt() {
    let error = extension_error(Some("after_dispatch"));
    assert_eq!(error.dispatch_stage(), BridgeDispatchStage::AfterDispatch);
    assert_delivered_wire(
        &through_remote_click(execute_cdp_click_bridge_mapping(&error)).await,
        "execute_cdp_click bridge",
    );
    // target_act DOM / coordinate / cdpInput lanes.
    assert_delivered_wire(
        &remote_bridge_click_error(&error),
        "remote_bridge_click_error",
    );
}

#[tokio::test]
async fn must_pass_extension_error_without_a_stage_stays_unknown_without_a_delivered_attempt() {
    let error = extension_error(None);
    assert_eq!(error.dispatch_stage(), BridgeDispatchStage::Unreported);
    assert_unknown_delivery_has_no_delivered_attempt(
        &through_remote_click(execute_cdp_click_bridge_mapping(&error)).await,
        "execute_cdp_click bridge",
    );
    assert_unknown_delivery_has_no_delivered_attempt(
        &remote_bridge_click_error(&error),
        "remote_bridge_click_error",
    );
}

#[tokio::test]
async fn must_pass_extension_error_before_dispatch_stays_refused_without_attempts() {
    let error = extension_error(Some("not_started"));
    let refused = remote_bridge_click_error(&error);
    crate::server::dispatch_status_189_tests::assert_not_delivered(
        &refused,
        "refused_before_delivery",
    );
    assert!(delivered_attempts(&refused).is_empty());
}
