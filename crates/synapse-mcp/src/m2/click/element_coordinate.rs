use super::*;

pub(crate) trait CoordinateClickPort: Send {
    fn point(&mut self, foreground: bool) -> Result<Point, ErrorData>;
    fn requires_foreground(&mut self) -> Result<bool, ErrorData>;
    fn authorize(&mut self) -> Result<(), ErrorData>;
    fn post(
        &mut self,
        point: Point,
    ) -> impl std::future::Future<Output = Result<String, ErrorData>> + Send;
    fn foreground(
        &mut self,
        point: Point,
        attempts: &mut Vec<ActClickTierAttempt>,
    ) -> impl std::future::Future<Output = Result<String, ErrorData>> + Send;
}

struct LiveCoordinateClick<'a> {
    handle: ActionHandle,
    params: &'a ActClickParams,
    element: &'a ActClickElementTarget,
    recording: Option<&'a RecordingBackend>,
    timing: DoubleClickTiming,
    policy: ForegroundClickPolicy,
    trace: &'static str,
}

impl CoordinateClickPort for LiveCoordinateClick<'_> {
    fn point(&mut self, foreground: bool) -> Result<Point, ErrorData> {
        let point = element_center(
            &self.element.element_id,
            self.recording.is_some(),
            foreground,
        )?;
        trace_element_click_outcome(
            self.element,
            0,
            self.trace,
            ElementClickTraceReadback {
                fallback_screen_point: Some(point),
                ..ElementClickTraceReadback::default()
            },
        );
        Ok(point)
    }
    fn requires_foreground(&mut self) -> Result<bool, ErrorData> {
        if self.recording.is_some() {
            return Ok(true);
        }
        let Some(metadata) = coordinate_click_foreground_requirement(&self.element.element_id)?
        else {
            return Ok(false);
        };
        trace_caret_route(self.element, &metadata);
        // #107: a claimed target that is not the OS foreground is posted to,
        // never sent to SendInput (which would refuse or use the human's cursor).
        let foreground_root = synapse_a11y::foreground_root_identity()
            .ok()
            .map(|identity| identity.root_hwnd);
        Ok(
            coordinate_tier(true, self.policy.claimed_root(), foreground_root)
                == CoordinateTier::Foreground,
        )
    }
    fn authorize(&mut self) -> Result<(), ErrorData> {
        if self.recording.is_some() {
            return Ok(());
        }
        self.policy.authorize_delivery(None)
    }
    async fn post(&mut self, point: Point) -> Result<String, ErrorData> {
        post_element_window_message_click(self.params, self.element, point, self.timing).await
    }
    async fn foreground(
        &mut self,
        point: Point,
        attempts: &mut Vec<ActClickTierAttempt>,
    ) -> Result<String, ErrorData> {
        let actions = coordinate_click_actions(self.params, point);
        if let Some(recording) = self.recording {
            record::execute_recording(recording, &actions, self.params.clicks, self.timing).await?;
        } else {
            let _lease = acquire_click_foreground_lease(
                &self.policy,
                self.params.hold_ms,
                attempts,
                Some(point),
            )?;
            record::execute_actor_actions(self.handle.clone(), actions, self.timing).await?;
        }
        Ok(backend_used_name(self.params.backend).to_owned())
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_coordinate_element_click(
    handle: ActionHandle,
    params: &ActClickParams,
    element: &ActClickElementTarget,
    recording: Option<&RecordingBackend>,
    timing: DoubleClickTiming,
    started: Instant,
    prior: Vec<ActClickTierAttempt>,
    trace: &'static str,
    policy: ForegroundClickPolicy,
) -> Result<ActClickResponse, ErrorData> {
    let mut port = LiveCoordinateClick {
        handle,
        params,
        element,
        recording,
        timing,
        policy,
        trace,
    };
    coordinate_click_with(&mut port, params, timing, started, prior).await
}

pub(crate) async fn coordinate_click_with(
    port: &mut impl CoordinateClickPort,
    params: &ActClickParams,
    timing: DoubleClickTiming,
    started: Instant,
    mut attempts: Vec<ActClickTierAttempt>,
) -> Result<ActClickResponse, ErrorData> {
    // #189 R4: a failed caret-route read is a terminal refusal.
    let foreground = port
        .requires_foreground()
        .map_err(|error| attach_click_tier_attempts(gate_read_failed(error), attempts.clone()))?;
    if !foreground {
        let point = port.point(false).map_err(|error| {
            postmessage_failure(mark_click_not_dispatched(error), attempts.clone())
        })?;
        match port.post(point).await {
            Ok(backend) => {
                attempts.push(click_tier_delivered(
                    CLICK_TIER_POSTMESSAGE,
                    false,
                    "HWND PostMessage click delivered",
                ));
                return Ok(click_response(
                    params, timing, started, backend, false, attempts,
                ));
            }
            Err(error) => {
                let error = postmessage_failure(error, attempts);
                if click_router_stopped(&error)
                    || !error
                        .data
                        .as_ref()
                        .is_some_and(|data| data["dispatch_not_started"] == true)
                {
                    return Err(error);
                }
                trace_postmessage_before_foreground(params, &error);
                attempts = click_attempts(&error);
            }
        }
    }
    coordinate_foreground_with(port, params, timing, started, attempts).await
}

async fn coordinate_foreground_with(
    port: &mut impl CoordinateClickPort,
    params: &ActClickParams,
    timing: DoubleClickTiming,
    started: Instant,
    mut attempts: Vec<ActClickTierAttempt>,
) -> Result<ActClickResponse, ErrorData> {
    // Authorization precedes scrolling/actionability work and is checked again at the lease.
    // Both refusals precede any input (#189 F1/F3).
    port.authorize()
        .map_err(|error| foreground_failure(mark_click_not_dispatched(error), attempts.clone()))?;
    let point = port
        .point(true)
        .map_err(|error| foreground_failure(mark_click_not_dispatched(error), attempts.clone()))?;
    match port.foreground(point, &mut attempts).await {
        Ok(backend) => {
            attempts.push(click_tier_delivered(
                CLICK_TIER_FOREGROUND,
                true,
                "leased foreground click delivered",
            ));
            Ok(click_response(
                params, timing, started, backend, false, attempts,
            ))
        }
        Err(error) => Err(foreground_failure(error, attempts)),
    }
}

fn foreground_failure(error: ErrorData, mut attempts: Vec<ActClickTierAttempt>) -> ErrorData {
    // Lease acquisition already attaches its own refusal attempt.
    let existing = click_attempts(&error);
    if !existing.is_empty() {
        return error;
    }
    if error
        .data
        .as_ref()
        .is_some_and(|data| data["delivered"] == true)
    {
        attempts.push(click_tier_delivered(
            CLICK_TIER_FOREGROUND,
            true,
            "foreground input delivered before failure",
        ));
    }
    attempts.push(click_tier_failed(
        CLICK_TIER_FOREGROUND,
        click_reason_for_error_code(&click_error_code(&error)),
        click_error_code(&error),
        true,
        error.message.to_string(),
    ));
    attach_click_tier_attempts(error, attempts)
}

fn coordinate_click_actions(params: &ActClickParams, screen_point: Point) -> Vec<Action> {
    let mut actions = Vec::with_capacity(usize::from(params.clicks) + 1);
    actions.push(Action::MouseMove {
        to: MouseTarget::Screen {
            point: screen_point,
        },
        curve: params.velocity_profile.to_aim_curve(),
        duration_ms: params.duration_ms,
        backend: params.backend,
    });
    for _ in 0..params.clicks {
        actions.push(Action::MouseButton {
            button: params.button,
            action: ButtonAction::Press,
            hold_ms: params.hold_ms,
            backend: params.backend,
        });
    }
    actions
}
