use super::*;
use crate::server::dispatch_status_189_tests::{assert_not_delivered, assert_verify_needed};
use std::cell::Cell;

fn failure() -> ErrorData {
    mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        "remote reply lost after click",
    )
}

#[tokio::test]
async fn must_fail_remote_reply_loss_retains_one_dispatch_and_requires_verification() {
    let deliveries = Cell::new(0);
    let result: Result<(), _> = remote_click_with(async {
        deliveries.set(deliveries.get() + 1);
        Err(failure())
    })
    .await;
    let error = result.expect_err("remote reply loss");
    assert_eq!(deliveries.get(), 1);
    assert_verify_needed(&error);
    assert_eq!(error.data.as_ref().unwrap()["possibly_delivered"], true);
    assert_ne!(error.data.as_ref().unwrap()["delivered"], true);
    assert_eq!(click_attempts(&error).len(), 1);
    assert_eq!(
        error.data.as_ref().unwrap()["router_escalation_allowed"],
        false
    );
}

#[tokio::test]
async fn must_fail_remote_readback_and_audit_keep_the_completed_control_delivery() {
    let deliveries = Cell::new(0);
    let result = remote_click_with(async {
        deliveries.set(deliveries.get() + 1);
        Ok::<_, ErrorData>(())
    })
    .await;
    for error in [
        remote_click_error(failure(), true),
        carry_remote_click_delivery(failure(), &result),
    ] {
        assert_verify_needed(&error);
        assert_eq!(error.data.as_ref().unwrap()["delivered"], true);
        assert_eq!(
            click_attempts(&error)
                .iter()
                .filter(|a| a.status == "delivered")
                .count(),
            1
        );
    }
    assert_eq!(deliveries.get(), 1);
}

#[tokio::test]
async fn must_pass_remote_predispatch_refusal_keeps_zero_deliveries() {
    let deliveries = Cell::new(0);
    let result: Result<(), _> = remote_click_with(async {
        let error = super::super::element::mark_click_not_dispatched(failure());
        if error.data.as_ref().unwrap()["dispatch_not_started"] == true {
            return Err(error);
        }
        deliveries.set(deliveries.get() + 1);
        Ok(())
    })
    .await;
    let error = result.expect_err("proved before dispatch");
    assert_eq!(deliveries.get(), 0);
    assert_ne!(error.data.as_ref().unwrap()["possibly_delivered"], true);
    assert_eq!(error.data.as_ref().unwrap()["dispatch_not_started"], true);
}

#[tokio::test]
async fn must_fail_remote_audit_after_lost_reply_keeps_unknown_delivery() {
    let deliveries = Cell::new(0);
    let result: Result<(), _> = remote_click_with(async {
        deliveries.set(deliveries.get() + 1);
        Err(failure())
    })
    .await;
    let error = carry_remote_click_delivery(
        mcp_error(error_codes::TOOL_INTERNAL_ERROR, "audit unavailable"),
        &result,
    );
    assert_verify_needed(&error);
    assert_eq!(deliveries.get(), 1);
    assert_eq!(error.data.as_ref().unwrap()["possibly_delivered"], true);
    assert_ne!(error.data.as_ref().unwrap()["delivered"], true);
    assert_eq!(click_attempts(&error).len(), 1);
}

#[tokio::test]
async fn must_pass_disabled_bridge_refusal_preserves_predispatch_evidence() {
    let result: Result<(), _> = remote_click_with(async {
        // Same refusal emitted by normal_bridge_attach_disabled before enqueue.
        Err(mcp_error(error_codes::A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED,
            "Synapse Chrome Bridge refused unsupported attach-capable command clickNode before queueing any Chrome command"))
    }).await;
    let error = result.expect_err("disabled bridge lane");
    assert_eq!(error.data.as_ref().unwrap()["dispatch_not_started"], true);
    assert_ne!(error.data.as_ref().unwrap()["possibly_delivered"], true);
}

#[test]
fn must_fail_browser_click_followups_discarding_delivery_evidence() {
    let source = include_str!("../../../server/background_router.rs").replace("\r\n", "\n");
    let body = source
        .split_once("async fn target_act_browser_dom_action(")
        .expect("DOM adapter exists")
        .1
        .split("\n}\n")
        .next()
        .expect("body");
    assert!(body.contains("let is_click = target_act_is_click_like_dom_action(action)"));
    assert!(
        body.contains("crate::m2::remote_bridge_click_error(&error)"),
        "reply-loss boundary carries the bridge dispatch stage"
    );
    assert_eq!(
        body.matches("crate::m2::remote_click_error(error, true)")
            .count(),
        2,
        "#189: popup registration and redaction follow a delivered click"
    );
    assert!(
        body.contains("crate::m2::carry_remote_click_delivery(error, &result)"),
        "audit boundary"
    );
}

/// The DOM/bridge boundary as composed by `remote_bridge_click_error` for
/// target_act_browser_dom_action and the bridge coordinate/cdpInput lanes:
/// bridge error -> mcp_error -> producer dispatch stage -> remote_click_error.
fn bridge_click_failure_at(
    code: &'static str,
    stage: crate::chrome_debugger_bridge::BridgeDispatchStage,
) -> ErrorData {
    crate::m2::remote_click_error(
        stamp_bridge_dispatch_stage(
            mcp_error(code, format!("synthetic bridge failure {code}")),
            stage,
        ),
        false,
    )
}

fn bridge_click_failure(code: &'static str) -> ErrorData {
    bridge_click_failure_at(
        code,
        crate::chrome_debugger_bridge::BridgeDispatchStage::Unreported,
    )
}

/// B8: the extension's catch-all after dispatched events is never a refusal;
/// only an explicit not_started stage proves it pre-dispatch.
#[test]
fn must_fail_dom_action_error_after_events_is_never_refused_before_delivery() {
    use crate::chrome_debugger_bridge::BridgeDispatchStage;
    let code = error_codes::CHROME_DOM_ACTION_UNSUPPORTED;
    let after = bridge_click_failure_at(code, BridgeDispatchStage::AfterDispatch);
    assert_verify_needed(&after);
    assert_eq!(after.data.as_ref().unwrap()["delivered"], true);
    let unreported = bridge_click_failure_at(code, BridgeDispatchStage::Unreported);
    assert_verify_needed(&unreported);
    assert_eq!(
        unreported.data.as_ref().unwrap()["possibly_delivered"],
        true
    );
    let resolver_only = bridge_click_failure_at(code, BridgeDispatchStage::NotStarted);
    assert_not_delivered(&resolver_only, "refused_before_delivery");
}

/// B2: a pre-enqueue bridge failure stamped not_started stays undelivered,
/// although its code (A11Y_CDP_ATTACH_FAILED) is not a pre-dispatch code.
#[test]
fn must_fail_lock_poisoned_enqueue_stays_undelivered() {
    let error = bridge_click_failure_at(
        error_codes::A11Y_CDP_ATTACH_FAILED,
        crate::chrome_debugger_bridge::BridgeDispatchStage::NotStarted,
    );
    assert_not_delivered(&error, "failed_before_delivery");
}

/// C4/C6: mouseMoved failure is pre-button; mouseReleased after a returned
/// mousePressed is delivered.
#[tokio::test]
async fn must_pass_raw_cdp_move_failure_is_undelivered_and_release_failure_delivered() {
    let source = |delivered: Option<bool>| {
        let inner = Box::new(synapse_a11y::A11yError::CdpAxtreeFailed {
            detail: "CDP input dispatch failed: synthetic".to_owned(),
        });
        match delivered {
            None => synapse_a11y::A11yError::ClickNotDispatched { source: inner },
            Some(delivered) => synapse_a11y::A11yError::ClickDispatch {
                source: inner,
                delivered,
            },
        }
    };
    let failure = |source: synapse_a11y::A11yError| async move {
        remote_click_with(async {
            Err::<(), _>(remote_cdp_click_failure(
                &source,
                mcp_error(source.code(), source.to_string()),
            ))
        })
        .await
        .expect_err("click event failure")
    };
    assert_not_delivered(&failure(source(None)).await, "failed_before_delivery");
    let released = failure(source(Some(true))).await;
    assert_verify_needed(&released);
    assert_eq!(released.data.as_ref().unwrap()["delivered"], true);
    let pressed = failure(source(Some(false))).await;
    assert_eq!(pressed.data.as_ref().unwrap()["possibly_delivered"], true);
    assert_ne!(pressed.data.as_ref().unwrap()["delivered"], true);
}

#[test]
fn must_fail_dom_locator_and_actionability_failures_stay_refused_before_delivery() {
    // Every producer of these codes runs before the first pointer event.
    for (code, state) in [
        (
            error_codes::CHROME_DOM_ELEMENT_NOT_FOUND,
            "refused_before_delivery",
        ),
        (
            error_codes::CHROME_DOM_SELECTOR_INVALID,
            "refused_before_delivery",
        ),
        (
            error_codes::CHROME_DOM_ELEMENT_AMBIGUOUS,
            "refused_before_delivery",
        ),
        (
            error_codes::CHROME_DOM_ELEMENT_NOT_ACTIONABLE,
            "refused_before_delivery",
        ),
        (error_codes::BROWSER_WAIT_TIMEOUT, "failed_before_delivery"),
    ] {
        let error = bridge_click_failure(code);
        assert_not_delivered(&error, state);
        assert!(
            click_attempts(&error).is_empty(),
            "{code}: no CDP attempt ran"
        );
        assert_ne!(
            error.data.as_ref().unwrap()["router_escalation_allowed"],
            false,
            "{code}: a refusal is not a delivery stop"
        );
    }
}

#[test]
fn must_fail_bridge_failures_before_enqueue_stay_undelivered() {
    for (code, state) in [
        (
            error_codes::A11Y_CDP_EXTENSION_UNAVAILABLE,
            "failed_before_delivery",
        ),
        (
            error_codes::CHROME_BRIDGE_EXTENSION_STALE,
            "failed_before_delivery",
        ),
        (
            error_codes::BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED,
            "failed_before_delivery",
        ),
        (error_codes::TOOL_PARAMS_INVALID, "refused_before_delivery"),
    ] {
        assert_not_delivered(&bridge_click_failure(code), state);
    }
}

async fn raw_point_click_failure(
    target_id: &str,
    point: synapse_a11y::CdpActionPoint,
    clicks: i64,
) -> ErrorData {
    // Same mapping as target_act_raw_cdp_coordinate_click.
    remote_click_with(async {
        synapse_a11y::cdp_click_point_target(
            "synapse-189-unused-endpoint",
            target_id,
            point,
            synapse_a11y::CdpMouseButton::Left,
            clicks,
            0,
        )
        .await
        .map_err(|error| {
            remote_cdp_click_failure(
                &error,
                mcp_error(
                    error.code(),
                    format!("raw-CDP coordinate click failed: {error}"),
                ),
            )
        })
    })
    .await
    .expect_err("raw-CDP click must fail before dispatch")
}

#[tokio::test]
async fn must_fail_raw_cdp_point_and_target_failures_never_claim_delivery() {
    let visible = synapse_a11y::CdpActionPoint { x: 10.0, y: 20.0 };
    let not_finite = synapse_a11y::CdpActionPoint {
        x: f64::NAN,
        y: 20.0,
    };
    // Real producer: target id, point and count are refused before any connect.
    for error in [
        raw_point_click_failure("", visible, 1).await,
        raw_point_click_failure("synapse-189-target", not_finite, 1).await,
        raw_point_click_failure("synapse-189-target", visible, 0).await,
    ] {
        assert_not_delivered(&error, "failed_before_delivery");
    }
}

#[tokio::test]
async fn must_fail_raw_cdp_node_click_on_missing_endpoint_never_claims_delivery() {
    // Same mapping as execute_cdp_click / target_act_raw_cdp_click_dispatch.
    let call = remote_click_with(async {
        synapse_a11y::cdp_click_node(
            "synapse-189-missing-endpoint",
            "",
            Some("synapse-189-target"),
            189,
            synapse_a11y::CdpMouseButton::Left,
            1,
            0,
        )
        .await
        .map_err(|error| {
            remote_cdp_click_failure(&error, mcp_error(error.code(), error.to_string()))
        })
    });
    let error = tokio::time::timeout(std::time::Duration::from_secs(20), call)
        .await
        .expect("an unparseable endpoint fails without waiting on a browser")
        .expect_err("missing endpoint cannot click");
    assert_eq!(
        click_error_code(&error),
        error_codes::A11Y_CDP_ATTACH_FAILED
    );
    assert_not_delivered(&error, "failed_before_delivery");
}

#[tokio::test]
async fn must_pass_raw_cdp_input_dispatch_failure_requires_verification() {
    let source = synapse_a11y::A11yError::ClickDispatch {
        source: Box::new(synapse_a11y::A11yError::CdpAxtreeFailed {
            detail: "CDP input dispatch failed: synthetic mousePressed reply loss".to_owned(),
        }),
        delivered: false,
    };
    let result: Result<(), _> = remote_click_with(async {
        Err(remote_cdp_click_failure(
            &source,
            mcp_error(source.code(), source.to_string()),
        ))
    })
    .await;
    let error = result.expect_err("dispatch failure");
    assert_verify_needed(&error);
    assert_eq!(error.data.as_ref().unwrap()["possibly_delivered"], true);
    assert_ne!(error.data.as_ref().unwrap()["dispatch_not_started"], true);
    assert_eq!(click_attempts(&error).len(), 1);
}

#[test]
fn must_pass_extension_reply_timeout_requires_verification() {
    // B3/B4: the command was queued; the extension may have clicked before the
    // reply timed out or the response channel closed.
    for code in [
        error_codes::A11Y_CDP_EXTENSION_TIMEOUT,
        error_codes::A11Y_CDP_ATTACH_FAILED,
    ] {
        let error = bridge_click_failure(code);
        assert_verify_needed(&error);
        assert_eq!(error.data.as_ref().unwrap()["possibly_delivered"], true);
        assert_eq!(
            error.data.as_ref().unwrap()["router_escalation_allowed"],
            false
        );
    }
}

#[tokio::test]
async fn must_fail_followup_error_code_never_proves_predispatch() {
    let result: Result<(), _> = remote_click_with(async { Err(failure()) }).await;
    // A later audit/redaction error carrying a pre-dispatch code says nothing
    // about the click that may already have happened.
    let error = carry_remote_click_delivery(
        mcp_error(error_codes::TOOL_PARAMS_INVALID, "audit row rejected"),
        &result,
    );
    assert_verify_needed(&error);
    assert_eq!(error.data.as_ref().unwrap()["possibly_delivered"], true);
    assert_ne!(error.data.as_ref().unwrap()["dispatch_not_started"], true);
}

fn source_body<'a>(source: &'a str, start: &str) -> &'a str {
    source
        .split_once(start)
        .expect("producer exists")
        .1
        .split_once("\n}\n")
        .expect("producer body ends")
        .0
}

#[test]
fn must_fail_raw_cdp_click_producers_or_lanes_losing_dispatch_typing() {
    let a11y = include_str!("../../../../../synapse-a11y/src/cdp_action.rs").replace("\r\n", "\n");
    for producer in [
        "pub async fn cdp_click_node(",
        "pub async fn cdp_click_point_target(",
    ] {
        let body = source_body(&a11y, producer);
        assert!(
            body.contains("ClickEventStage::"),
            "#189: {producer} types input failures by event stage"
        );
        assert!(
            !body.contains(" dispatch_err(&"),
            "#189: {producer} has an untyped input failure"
        );
    }
    let router = include_str!("../../../server/background_router.rs").replace("\r\n", "\n");
    for lane in [
        "async fn target_act_raw_cdp_coordinate_click(",
        "async fn target_act_raw_cdp_click_dispatch(",
    ] {
        let body = source_body(&router, lane);
        assert!(
            body.contains("crate::m2::remote_cdp_click_failure("),
            "#189: {lane} classifies raw-CDP failures"
        );
    }
    let click = include_str!("../../click.rs").replace("\r\n", "\n");
    let body = source_body(&click, "async fn execute_cdp_click(");
    assert!(
        body.contains("remote_cdp_click_failure("),
        "#189: execute_cdp_click classifies raw-CDP failures"
    );
}
