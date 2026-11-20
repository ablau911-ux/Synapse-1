use super::*;

/// Metadata and native operations are injected; the live entry and tests share
/// the loop and every fallback decision.
pub(crate) trait ElementClickPort: Send {
    fn metadata(&mut self) -> Result<synapse_a11y::ElementMetadataReadback, ErrorData>;
    fn click(&mut self) -> Result<ElementClickOutcome, ActionError>;
    fn coordinate(
        &mut self,
        prior: Vec<ActClickTierAttempt>,
        trace: &'static str,
    ) -> impl std::future::Future<Output = Result<ActClickResponse, ErrorData>> + Send;
    /// A recording session records input instead of sending it (#189 U9 gate).
    fn records_input(&self) -> bool {
        false
    }
}

struct LiveElementClick<'a> {
    handle: ActionHandle,
    params: &'a ActClickParams,
    element: &'a ActClickElementTarget,
    recording: Option<&'a RecordingBackend>,
    timing: DoubleClickTiming,
    started: Instant,
    policy: ForegroundClickPolicy,
    state: EmitState,
}

impl ElementClickPort for LiveElementClick<'_> {
    fn metadata(&mut self) -> Result<synapse_a11y::ElementMetadataReadback, ErrorData> {
        element_coordinate_fallback_metadata(&self.element.element_id)
    }

    fn click(&mut self) -> Result<ElementClickOutcome, ActionError> {
        if let Some(recording) = self.recording {
            click_element_or_fallback(
                &self.element.element_id,
                recording,
                &mut self.state,
                self.params.button,
            )
        } else {
            click_element_or_fallback(
                &self.element.element_id,
                &synapse_action::backend::software::SoftwareBackend::new(),
                &mut self.state,
                self.params.button,
            )
        }
    }

    async fn coordinate(
        &mut self,
        prior: Vec<ActClickTierAttempt>,
        trace: &'static str,
    ) -> Result<ActClickResponse, ErrorData> {
        execute_coordinate_element_click(
            self.handle.clone(),
            self.params,
            self.element,
            self.recording,
            self.timing,
            self.started,
            prior,
            trace,
            self.policy.clone(),
        )
        .await
    }

    fn records_input(&self) -> bool {
        self.recording.is_some()
    }
}

pub(crate) async fn execute_element_click(
    handle: ActionHandle,
    params: &ActClickParams,
    element: &ActClickElementTarget,
    recording: Option<&RecordingBackend>,
    timing: DoubleClickTiming,
    started: Instant,
    foreground_click_policy: ForegroundClickPolicy,
) -> Result<ActClickResponse, ErrorData> {
    let mut port = LiveElementClick {
        handle,
        params,
        element,
        recording,
        timing,
        started,
        policy: foreground_click_policy,
        state: EmitState::new(),
    };
    execute_element_click_with(&mut port, params, timing, started).await
}

pub(crate) async fn execute_element_click_with(
    port: &mut impl ElementClickPort,
    params: &ActClickParams,
    timing: DoubleClickTiming,
    started: Instant,
) -> Result<ActClickResponse, ErrorData> {
    let super::super::ActClickTarget::Element(element) = &params.target else {
        return Err(mark_click_not_dispatched(crate::m1::mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "element target required",
        )));
    };
    // #2326: route memory sets `coordinate_first` when learned evidence
    // promotes the posted coordinate tier over UIA for this control.
    if element_is_coordinate_only(&element.element_id)
        || !params.use_invoke_pattern
        || params.coordinate_first
    {
        return port.coordinate(Vec::new(), "coordinate_direct").await;
    }
    let metadata = port.metadata().map_err(gate_read_failed)?;
    if coordinate_fallback_required_for_selection_only_list_item(&metadata) {
        return selection_only_route(port, params, element, &metadata).await;
    }
    match run_uia_clicks(port, params, element, timing).await {
        Ok(outcomes) => Ok(click_response(
            params,
            timing,
            started,
            "uia".to_owned(),
            true,
            vec![click_tier_delivered(
                CLICK_TIER_UIA,
                false,
                format!(
                    "UI Automation semantic click delivered; outcomes={}",
                    outcomes.join(",")
                ),
            )],
        )),
        Err(failure) => resolve_element_failure(port, params, element, failure).await,
    }
}

async fn selection_only_route(
    port: &mut impl ElementClickPort,
    params: &ActClickParams,
    element: &ActClickElementTarget,
    metadata: &synapse_a11y::ElementMetadataReadback,
) -> Result<ActClickResponse, ErrorData> {
    let detail = "UIA SelectionItemPattern on list item is selection-only and does not reliably emit the item click/activation event required by act_click";
    let attempts = vec![click_tier_failed(
        CLICK_TIER_UIA,
        CLICK_REASON_SELECTION_ONLY,
        error_codes::ACTION_TARGET_INVALID,
        false,
        detail,
    )];
    trace_selection_only_route(element, metadata, params.coordinate_fallback_on_unsupported);
    if params.coordinate_fallback_on_unsupported {
        port.coordinate(attempts, "coordinate_fallback_on_selection_only_list_item")
            .await
    } else {
        Err(attach_click_tier_attempts(
            mark_click_not_dispatched(selection_only_list_item_error(&element.element_id, detail)),
            attempts,
        ))
    }
}

struct UiaClickFailure {
    error: ActionError,
    index: u8,
    outcomes: Vec<String>,
    unexpected_plan: bool,
}

async fn run_uia_clicks(
    port: &mut impl ElementClickPort,
    params: &ActClickParams,
    element: &ActClickElementTarget,
    timing: DoubleClickTiming,
) -> Result<Vec<String>, UiaClickFailure> {
    let mut outcomes = Vec::new();
    for index in 0..params.clicks {
        let outcome = port.click();
        let unexpected_plan = matches!(&outcome, Ok(ElementClickOutcome::CoordinateFallback(_)));
        let result = outcome.and_then(|outcome| semantic_outcome(element, index, outcome));
        match result {
            Ok(outcome) => outcomes.push(outcome.to_owned()),
            Err(error) => {
                return Err(UiaClickFailure {
                    error,
                    index,
                    outcomes,
                    unexpected_plan,
                });
            }
        }
        if index + 1 < params.clicks {
            sleep(Duration::from_millis(u64::from(
                timing.inter_click_delay_ms,
            )))
            .await;
        }
    }
    Ok(outcomes)
}

async fn resolve_element_failure(
    port: &mut impl ElementClickPort,
    params: &ActClickParams,
    element: &ActClickElementTarget,
    failure: UiaClickFailure,
) -> Result<ActClickResponse, ErrorData> {
    let UiaClickFailure {
        error,
        index,
        outcomes,
        unexpected_plan,
    } = failure;
    if let Some(stopped) = uia_click_error_after_dispatch(&error, index, params.clicks, &outcomes) {
        return Err(stopped);
    }
    // Typed as not dispatched from here on: the converted error says so.
    let attempts = vec![click_tier_failed(
        CLICK_TIER_UIA,
        click_reason_for_error_code(error.code()),
        error.code(),
        false,
        error.detail(),
    )];
    if unexpected_plan {
        return Err(stop_click_router(
            attach_click_tier_attempts(action_error_to_mcp(&error), attempts),
            "unexpected_coordinate_plan",
        ));
    }
    if error.code() == error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED {
        return unsupported_element_route(port, params, element, &error, attempts).await;
    }
    if uia_tier_unavailable(&error) && worker_admission_fallback_allowed(port, params) {
        trace_worker_admission_route(element, &error);
        return port
            .coordinate(attempts, "coordinate_fallback_on_uia_worker_admission")
            .await;
    }
    Err(attach_click_tier_attempts(
        mark_click_not_dispatched(action_error_to_mcp(&error)),
        attempts,
    ))
}

/// #189 U9: the one in-tier route for a refused UIA worker admission, under
/// the same eligibility the background-first click route always required.
fn worker_admission_fallback_allowed(
    port: &impl ElementClickPort,
    params: &ActClickParams,
) -> bool {
    !port.records_input() && super::super::click_params_can_route_background_first(params)
}

fn trace_worker_admission_route(element: &ActClickElementTarget, error: &ActionError) {
    tracing::warn!(
        code = "M2_ACT_CLICK_UIA_WORKER_ADMISSION_FALLBACK",
        kind = "act_click",
        element_id = %element.element_id,
        detail = error.detail(),
        "UIA worker refused admission before any dispatch; routing once through the coordinate tier"
    );
}

async fn unsupported_element_route(
    port: &mut impl ElementClickPort,
    params: &ActClickParams,
    element: &ActClickElementTarget,
    error: &ActionError,
    attempts: Vec<ActClickTierAttempt>,
) -> Result<ActClickResponse, ErrorData> {
    if !params.coordinate_fallback_on_unsupported {
        return Err(attach_click_tier_attempts(
            element_pattern_unsupported_coordinate_fallback_disabled_error(
                &element.element_id,
                error.detail(),
            ),
            attempts,
        ));
    }
    let metadata = port
        .metadata()
        .map_err(|error| attach_click_tier_attempts(gate_read_failed(error), attempts.clone()))?;
    let allowed = coordinate_fallback_allowed_for_metadata(&metadata);
    trace_unsupported_route(element, &metadata, allowed);
    if allowed {
        return port
            .coordinate(attempts, "coordinate_fallback_on_unsupported")
            .await;
    }
    Err(click_metadata_refusal_error(error))
}

fn semantic_outcome(
    element: &ActClickElementTarget,
    index: u8,
    outcome: ElementClickOutcome,
) -> Result<&'static str, ActionError> {
    let name = match &outcome {
        ElementClickOutcome::Invoked => "invoked",
        ElementClickOutcome::Toggled { .. } => "toggled",
        ElementClickOutcome::Selected { .. } => "selected",
        ElementClickOutcome::Expanded { .. } => "expanded",
        ElementClickOutcome::Collapsed { .. } => "collapsed",
        ElementClickOutcome::LegacyDefaultAction { .. } => "legacy_default_action",
        ElementClickOutcome::CoordinateFallback(plan) => {
            tracing::error!(
                code = "M2_ACT_CLICK_UNEXPECTED_COORDINATE_FALLBACK",
                kind = "act_click",
                element_id = %element.element_id,
                screen_x = plan.screen_point.x,
                screen_y = plan.screen_point.y,
                "semantic UIA element click returned a coordinate fallback plan; no fallback delivery attempted"
            );
            // Typed not dispatched: no semantic pattern call ran for this plan.
            return Err(ActionError::ClickNotDispatched {
                source: Box::new(ActionError::ElementPatternUnsupported {
                    element_id: element.element_id.clone(),
                    detail: format!(
                        "semantic UIA click unexpectedly returned coordinate plan {plan:?}; no fallback delivered"
                    ),
                }),
                tier_unavailable: false,
            });
        }
    };
    trace_semantic_outcome(element, index, name, &outcome);
    Ok(name)
}

pub(crate) fn click_response(
    params: &ActClickParams,
    timing: DoubleClickTiming,
    started: Instant,
    backend_used: String,
    used_invoke_pattern: bool,
    tier_attempts: Vec<ActClickTierAttempt>,
) -> ActClickResponse {
    ActClickResponse {
        ok: true,
        used_invoke_pattern,
        backend_used,
        backend_tier_used: click_backend_tier_used(&tier_attempts),
        required_foreground: click_required_foreground(&tier_attempts),
        tier_attempts,
        postcondition: postcondition_not_requested(),
        press_hold_ms: params.hold_ms,
        double_click_window_ms: timing.window_ms,
        inter_click_delay_ms: timing.inter_click_delay_ms,
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    }
}

#[cfg(test)]
#[path = "tests/worker_admission_recording_271.rs"]
mod worker_admission_recording_271;
