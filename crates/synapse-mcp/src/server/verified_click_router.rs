//! Shared verified-click router with injectable delivery and readback (#189/#190).
use super::*;

/// The router's two seams: one delivery call and its delta readback.
pub(super) trait ClickTierPort: Sync {
    fn deliver(
        &self,
        params: ActClickParams,
    ) -> impl std::future::Future<Output = Result<ActClickResponse, ErrorData>> + Send;
    fn verify(
        &self,
    ) -> impl std::future::Future<Output = Result<ActClickPostcondition, ErrorData>> + Send;
}

/// Live port: the real delivery entry plus `verify_click_delta`.
pub(super) struct LiveClickTierPort<'a> {
    pub(super) handle: ActionHandle,
    pub(super) recording: Option<Arc<RecordingBackend>>,
    pub(super) policy: ForegroundClickPolicy,
    pub(super) service: &'a SynapseService,
    pub(super) before: ClickDeltaSignature,
    pub(super) verify_timeout_ms: u32,
    pub(super) target_window_hwnd: Option<i64>,
}

impl ClickTierPort for LiveClickTierPort<'_> {
    async fn deliver(&self, params: ActClickParams) -> Result<ActClickResponse, ErrorData> {
        act_click_with_handle_and_lease(
            self.handle.clone(),
            self.recording.clone(),
            params,
            self.policy.clone(),
        )
        .await
    }

    async fn verify(&self) -> Result<ActClickPostcondition, ErrorData> {
        self.service
            .verify_click_delta(
                self.before.clone(),
                self.verify_timeout_ms,
                self.target_window_hwnd,
            )
            .await
    }
}

/// #189: one escalation owner. Every tier change (unsupported or caret
/// fallback, selection-only list item, UIA worker admission, PostMessage sent
/// nothing) happens inside the delivery call, each tier at most once. A
/// returned error is terminal here; only a read-only observation may upgrade
/// a delivered result to verified.
pub(super) async fn route_verified_click<P: ClickTierPort>(
    port: &P,
    params: ActClickParams,
    started: Instant,
) -> Result<ActClickResponse, ErrorData> {
    let result = port.deliver(params.clone()).await;
    verify_click_tier(port, &params, result, started).await
}

async fn verify_click_tier<P: ClickTierPort>(
    port: &P,
    params: &ActClickParams,
    result: Result<ActClickResponse, ErrorData>,
    started: Instant,
) -> Result<ActClickResponse, ErrorData> {
    match result {
        Ok(response) => finish_verified_click(
            response,
            port.verify().await,
            matches!(&params.target, crate::m2::ActClickTarget::Element(_)),
        ),
        Err(error) if click_postdispatch_readback_failed(&error) => match port.verify().await {
            Ok(postcondition) => SynapseService::reconcile_click_postdispatch_error(
                params,
                &error,
                postcondition,
                started,
            ),
            Err(verify_error) => {
                tracing::warn!(
                    code = click_error_data_code(&verify_error)
                        .unwrap_or(error_codes::ACTION_POSTCONDITION_FAILED),
                    kind = "act_click",
                    original_error_code = click_error_data_code(&error)
                        .unwrap_or(error_codes::ACTION_TARGET_INVALID),
                    detail = %verify_error.message,
                    "post-dispatch readback failed; click delivered but unverified"
                );
                Err(mark_input_delivered(error))
            }
        },
        Err(error) => Err(classify_unknown_click_delivery(error)),
    }
}

pub(super) fn finish_verified_click(
    mut response: ActClickResponse,
    verification: Result<ActClickPostcondition, ErrorData>,
    semantic_route: bool,
) -> Result<ActClickResponse, ErrorData> {
    match verification {
        Ok(postcondition) => {
            response.postcondition = postcondition;
            Ok(response)
        }
        Err(error) => {
            let error_code = click_error_data_code(&error)
                .unwrap_or(error_codes::ACTION_NO_OBSERVED_DELTA)
                .to_owned();
            // Dialog dismissal is an effect only for an observed semantic target.
            if semantic_route && error_code == error_codes::TARGET_WINDOW_NOT_FOUND {
                response.postcondition =
                    postcondition_target_window_closed("act_click", error.message.to_string());
                return Ok(response);
            }
            let mut attempts = response.tier_attempts;
            attempts.push(click_tier_failed(
                response.backend_tier_used,
                CLICK_REASON_NO_OBSERVED_DELTA,
                &error_code,
                response.required_foreground,
                error.message.to_string(),
            ));
            Err(mark_input_delivered(attach_click_tier_attempts(
                error, attempts,
            )))
        }
    }
}

pub(super) fn click_delivery_snapshot(
    result: &Result<ActClickResponse, ErrorData>,
) -> Result<Vec<ActClickTierAttempt>, ErrorData> {
    result
        .as_ref()
        .map(|response| response.tier_attempts.clone())
        .map_err(Clone::clone)
}

/// Observation, foreground readback and audit errors cannot erase earlier input.
pub(super) fn carry_click_delivery(
    error: ErrorData,
    dispatched: &Result<Vec<ActClickTierAttempt>, ErrorData>,
) -> ErrorData {
    let (attempts, delivered, possible) = match dispatched {
        Ok(attempts) => (attempts.clone(), true, false),
        Err(original) => {
            let attempts = click_tier_attempts_from_error(original);
            let delivered = click_tier_attempts_delivered(&attempts)
                || original
                    .data
                    .as_ref()
                    .is_some_and(|data| data["delivered"] == true);
            let possible = original
                .data
                .as_ref()
                .is_some_and(|data| data["possibly_delivered"] == true);
            (attempts, delivered, possible)
        }
    };
    if !delivered && !possible {
        return error;
    }
    let error = if click_tier_attempts_from_error(&error).is_empty() {
        attach_click_tier_attempts(error, attempts)
    } else {
        error
    };
    if delivered {
        mark_input_delivered(error)
    } else {
        crate::m2::postcondition::mark_input_possibly_delivered(error)
    }
}

/// #189 R5: producers stamp typed evidence. An error from the dispatching call
/// that carries none is unknown delivery, whatever its code says.
fn classify_unknown_click_delivery(error: ErrorData) -> ErrorData {
    let explicit = error.data.as_ref().is_some_and(|data| {
        data["dispatch_not_started"] == true
            || data["router_escalation_allowed"] == false
            || data["delivered"] == true
            || data["possibly_delivered"] == true
    });
    if explicit {
        error
    } else {
        crate::m2::postcondition::mark_input_possibly_delivered(error)
    }
}
