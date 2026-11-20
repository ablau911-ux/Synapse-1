//! Behavioral regression tests for the real element dispatch loop.
use super::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

struct Port {
    metadata: synapse_a11y::ElementMetadataReadback,
    script: VecDeque<(bool, Result<ElementClickOutcome, ActionError>)>,
    counts: Arc<Mutex<[usize; 2]>>,
}

impl ElementClickPort for Port {
    fn metadata(&mut self) -> Result<synapse_a11y::ElementMetadataReadback, ErrorData> {
        Ok(self.metadata.clone())
    }

    fn click(&mut self) -> Result<ElementClickOutcome, ActionError> {
        let (delivered, result) = self.script.pop_front().expect("one scripted UIA call");
        if delivered {
            self.counts.lock().unwrap()[0] += 1;
        }
        result
    }

    async fn coordinate(
        &mut self,
        mut prior: Vec<ActClickTierAttempt>,
        _: &'static str,
    ) -> Result<ActClickResponse, ErrorData> {
        self.counts.lock().unwrap()[1] += 1;
        prior.push(click_tier_delivered(
            CLICK_TIER_POSTMESSAGE,
            false,
            "coordinate delivery",
        ));
        Ok(click_response(
            &params(),
            DoubleClickTiming {
                window_ms: 500,
                inter_click_delay_ms: 0,
                source: "test",
            },
            Instant::now(),
            "postmessage".to_owned(),
            false,
            prior,
        ))
    }
}

/// The producer's typed shape for a failure before any pattern call.
fn not_dispatched(source: ActionError, tier_unavailable: bool) -> ActionError {
    ActionError::ClickNotDispatched {
        source: Box::new(source),
        tier_unavailable,
    }
}

fn unsupported(detail: &str) -> ActionError {
    not_dispatched(
        ActionError::ElementPatternUnsupported {
            element_id: synapse_core::element_id(189, "01020304"),
            detail: detail.to_owned(),
        },
        false,
    )
}

fn params() -> ActClickParams {
    serde_json::from_value(json!({"target":{"element_id":"0xbd:01020304"}})).unwrap()
}

fn port(role: &str, script: Vec<(bool, Result<ElementClickOutcome, ActionError>)>) -> Port {
    Port {
        metadata: synapse_a11y::ElementMetadataReadback {
            name: "control-189".to_owned(),
            value: None,
            role: role.to_owned(),
            automation_id: None,
            enabled: true,
            keyboard_focusable: true,
            patterns: vec![UiaPattern::SelectionItem],
            bbox: synapse_core::Rect {
                x: 10,
                y: 10,
                w: 20,
                h: 20,
            },
        },
        script: script.into(),
        counts: Arc::new(Mutex::new([0, 0])),
    }
}

async fn run(port: &mut Port, params: &ActClickParams) -> Result<ActClickResponse, ErrorData> {
    run_with(port, params).await
}

async fn run_with(
    port: &mut impl ElementClickPort,
    params: &ActClickParams,
) -> Result<ActClickResponse, ErrorData> {
    execute_element_click_with(
        port,
        params,
        DoubleClickTiming {
            window_ms: 500,
            inter_click_delay_ms: 0,
            source: "test",
        },
        Instant::now(),
    )
    .await
}

#[tokio::test]
async fn must_fail_radio_select_unchanged_never_activates_control_twice() {
    // resolve_click types a returned Select whose readback stayed false as delivered.
    let source = ActionError::ClickDispatch {
        delivered: true,
        source: Box::new(ActionError::TargetInvalid {
            detail: "SelectionItemPattern.select returned, but IsSelected stayed false".to_owned(),
        }),
    };
    let mut port = port("radio button", vec![(true, Err(source))]);
    let error = run(&mut port, &params())
        .await
        .expect_err("readback unverified");
    assert_eq!(
        *port.counts.lock().unwrap(),
        [1, 0],
        "#189: Select already activated this control"
    );
    assert_eq!(error.data.as_ref().unwrap()["delivered"], true);
}

#[tokio::test]
async fn must_fail_metadata_refusal_runs_real_refusal_branch_without_delivery() {
    let mut port = port("static text", vec![(false, Err(unsupported("no pattern")))]);
    port.metadata.keyboard_focusable = false;
    let error = run(&mut port, &params())
        .await
        .expect_err("metadata refusal");
    assert_eq!(*port.counts.lock().unwrap(), [0, 0]);
    assert_eq!(
        error.data.as_ref().unwrap()["router_stop_reason"],
        "coordinate_fallback_metadata_denied"
    );
}

#[tokio::test]
async fn must_fail_second_click_error_keeps_first_delivery_and_never_replays() {
    let mut port = port(
        "button",
        vec![
            (true, Ok(ElementClickOutcome::Invoked)),
            (
                false,
                Err(ActionError::BackendUnavailable {
                    detail: "phase=worker_busy".to_owned(),
                }),
            ),
        ],
    );
    let mut params = params();
    params.clicks = 2;
    let error = run(&mut port, &params).await.expect_err("partial sequence");
    assert_eq!(*port.counts.lock().unwrap(), [1, 0]);
    assert_eq!(error.data.as_ref().unwrap()["delivered"], true);
    assert_eq!(
        error.data.as_ref().unwrap()["tier_attempts"][0]["status"],
        "delivered"
    );
}

#[tokio::test]
async fn must_pass_semantic_success_activates_target_once() {
    let mut port = port("button", vec![(true, Ok(ElementClickOutcome::Invoked))]);
    let response = run(&mut port, &params()).await.expect("semantic delivery");
    assert_eq!(*port.counts.lock().unwrap(), [1, 0]);
    assert_eq!(response.backend_tier_used, "uia");
}

#[tokio::test]
async fn must_fail_all_pattern_provider_errors_stop_after_one_control_dispatch() {
    for pattern in ["Invoke", "Toggle", "Select", "Expand", "Collapse", "Legacy"] {
        let source = ActionError::ClickDispatch {
            delivered: false,
            source: Box::new(ActionError::TargetInvalid {
                detail: format!("{pattern}: opaque provider failure"),
            }),
        };
        let mut port = port("radio button", vec![(true, Err(source))]);
        let result = run(&mut port, &params()).await;
        assert_eq!(
            *port.counts.lock().unwrap(),
            [1, 0],
            "control-189/{pattern}"
        );
        let data = result
            .expect_err("provider completion unknown")
            .data
            .unwrap();
        assert_eq!(data["possibly_delivered"], true);
        assert_ne!(data["delivered"], true);
        assert_eq!(data["tier_attempts"][0]["reason_code"], "delivery_unknown");
    }
}

#[tokio::test]
async fn must_fail_all_returned_patterns_preserve_delivery_when_readback_errors() {
    for pattern in ["Toggle", "Select", "Expand", "Collapse"] {
        let source = ActionError::ClickDispatch {
            delivered: true,
            source: Box::new(ActionError::TargetInvalid {
                detail: format!("{pattern}: arbitrary readback failure"),
            }),
        };
        let mut port = port("radio button", vec![(true, Err(source))]);
        let result = run(&mut port, &params()).await;
        assert_eq!(
            *port.counts.lock().unwrap(),
            [1, 0],
            "control-189/{pattern}"
        );
        let data = result.expect_err("readback failed").data.unwrap();
        assert_eq!(data["delivered"], true);
        assert_eq!(data["click_dispatch_stage"], "readback");
        assert_eq!(data["tier_attempts"][0]["status"], "delivered");
    }
}

#[tokio::test]
async fn must_pass_unsupported_edit_and_selection_only_list_route_once_before_dispatch() {
    let mut edit = port(
        "edit",
        vec![(false, Err(unsupported("no supported pattern")))],
    );
    run(&mut edit, &params())
        .await
        .expect("eligible coordinate fallback");
    assert_eq!(*edit.counts.lock().unwrap(), [0, 1]);
    let mut list = port("list item", vec![]);
    run(&mut list, &params())
        .await
        .expect("skip selection-only semantic action");
    assert_eq!(*list.counts.lock().unwrap(), [0, 1]);
}

#[tokio::test]
async fn must_fail_fallback_disabled_preserves_refusal_without_control_delivery() {
    let mut port = port("edit", vec![(false, Err(unsupported("unsupported")))]);
    let mut params = params();
    params.coordinate_fallback_on_unsupported = false;
    let result = run(&mut port, &params).await;
    assert_eq!(*port.counts.lock().unwrap(), [0, 0]);
    assert_eq!(
        result.unwrap_err().data.unwrap()["router_escalation_allowed"],
        false
    );
}

/// U2: a failed metadata read is a terminal refusal with zero deliveries.
#[tokio::test]
async fn must_fail_metadata_read_error_is_refused_with_zero_deliveries() {
    struct FailingMetadata(Port);
    impl ElementClickPort for FailingMetadata {
        fn metadata(&mut self) -> Result<synapse_a11y::ElementMetadataReadback, ErrorData> {
            Err(crate::m1::mcp_error(
                error_codes::ACTION_ELEMENT_NOT_RESOLVED,
                "metadata read failed",
            ))
        }
        fn click(&mut self) -> Result<ElementClickOutcome, ActionError> {
            self.0.click()
        }
        async fn coordinate(
            &mut self,
            prior: Vec<ActClickTierAttempt>,
            trace: &'static str,
        ) -> Result<ActClickResponse, ErrorData> {
            self.0.coordinate(prior, trace).await
        }
    }
    let mut failing = FailingMetadata(port("button", vec![]));
    let error = run_with(&mut failing, &params())
        .await
        .expect_err("gate read");
    assert_eq!(*failing.0.counts.lock().unwrap(), [0, 0]);
    crate::server::dispatch_status_189_tests::assert_not_delivered(
        &error,
        "refused_before_delivery",
    );
    assert_eq!(
        error.data.unwrap()["router_stop_reason"],
        "gate_read_failed"
    );
}

/// U9: a refused UIA worker admission takes the in-tier coordinate route once.
#[tokio::test]
async fn must_pass_worker_admission_refusal_routes_in_tier_exactly_once() {
    for phase in ["worker_limit", "worker_busy", "worker_unavailable"] {
        let source = not_dispatched(
            ActionError::BackendUnavailable {
                detail: format!("phase={phase}"),
            },
            true,
        );
        let mut port = port("button", vec![(false, Err(source))]);
        let response = run(&mut port, &params()).await.expect("one fallback route");
        assert_eq!(*port.counts.lock().unwrap(), [0, 1], "{phase}");
        assert_eq!(response.backend_tier_used, CLICK_TIER_POSTMESSAGE);
        assert_eq!(response.tier_attempts[0].status, "failed");
    }
}

/// U9 gate: with the fallback disabled the admission refusal stays terminal.
#[tokio::test]
async fn must_fail_worker_admission_refusal_without_fallback_is_terminal() {
    let source = not_dispatched(
        ActionError::BackendUnavailable {
            detail: "phase=worker_busy".to_owned(),
        },
        true,
    );
    let mut port = port("button", vec![(false, Err(source))]);
    let mut params = params();
    params.coordinate_fallback_on_unsupported = false;
    let error = run(&mut port, &params).await.expect_err("no fallback");
    assert_eq!(*port.counts.lock().unwrap(), [0, 0]);
    crate::server::dispatch_status_189_tests::assert_not_delivered(
        &error,
        "failed_before_delivery",
    );
}

/// U10: a send/spawn/lock failure is undelivered and never escalates.
#[tokio::test]
async fn must_fail_worker_send_failure_never_escalates() {
    let source = not_dispatched(
        ActionError::TargetInvalid {
            detail: "send UIA worker job failed: receiver closed".to_owned(),
        },
        false,
    );
    let mut port = port("button", vec![(false, Err(source))]);
    let error = run(&mut port, &params()).await.expect_err("terminal");
    assert_eq!(*port.counts.lock().unwrap(), [0, 0]);
    let data = error.data.as_ref().unwrap();
    assert_eq!(data["dispatch_not_started"], true);
    assert_ne!(data["possibly_delivered"], true);
}

/// U18: a semantic click that returns a coordinate plan is a terminal refusal.
#[tokio::test]
async fn must_fail_unexpected_coordinate_plan_is_terminal_without_delivery() {
    let plan = synapse_action::CoordinateFallbackPlan {
        screen_point: Point { x: 20, y: 20 },
        window_point: Point { x: 10, y: 10 },
    };
    let mut port = port(
        "button",
        vec![(false, Ok(ElementClickOutcome::CoordinateFallback(plan)))],
    );
    let error = run(&mut port, &params()).await.expect_err("terminal plan");
    assert_eq!(*port.counts.lock().unwrap(), [0, 0]);
    let data = error.data.as_ref().unwrap();
    assert_eq!(data["router_stop_reason"], "unexpected_coordinate_plan");
    assert_eq!(data["dispatch_not_started"], true);
    assert_ne!(data["possibly_delivered"], true);
}

/// U16 (expand): a returned Expand whose state stayed is delivered once.
#[tokio::test]
async fn must_fail_expand_unchanged_never_activates_control_twice() {
    let source = ActionError::ClickDispatch {
        delivered: true,
        source: Box::new(ActionError::TargetInvalid {
            detail: "ExpandCollapsePattern.expand returned, but state stayed Collapsed".to_owned(),
        }),
    };
    let mut port = port("combo box", vec![(true, Err(source))]);
    let error = run(&mut port, &params()).await.expect_err("unverified");
    assert_eq!(*port.counts.lock().unwrap(), [1, 0]);
    let data = error.data.as_ref().unwrap();
    assert_eq!(data["delivered"], true);
    assert_eq!(data["click_dispatch_stage"], "readback");
}
