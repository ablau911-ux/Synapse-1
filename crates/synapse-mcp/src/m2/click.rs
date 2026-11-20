use std::{sync::Arc, time::Instant};

use rmcp::{ErrorData, model::ErrorCode};
use serde_json::{Map, Value, json};
use synapse_action::{ActionError, ActionHandle, RecordingBackend, cached_double_click_timing};
use synapse_core::{Action, Backend, ButtonAction, ElementId, MouseTarget, Point, error_codes};

use crate::m1::mcp_error;

mod element;
pub(crate) use element::click_metadata_refusal_error;
#[cfg(test)]
pub(crate) use element::dispatch_189_test_support;
#[cfg(windows)]
pub(crate) use element::hwnd_message_readback_for_root;
#[cfg_attr(not(windows), allow(dead_code))]
mod foreground_gate;
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod hit_test;
mod hwnd_post;
#[cfg_attr(not(windows), allow(dead_code))]
mod point_element;
mod point_post;
pub(crate) use point_post::{
    ClaimedPointTarget, PointClickRoute, claimed_background_point_possible,
    current_point_click_route,
    execute_claimed_point_postmessage_click as act_click_claimed_point_postmessage,
    point_click_resolves_root, unclaimed_point_target_error,
};
mod record;
mod remote_delivery;
pub(crate) use remote_delivery::{
    carry_remote_click_delivery, remote_bridge_click_error, remote_cdp_click_failure,
    remote_click_error, remote_click_with,
};
mod schema;
#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "click/tests/hit_test_197.rs"]
mod hit_test_197_tests;

#[cfg(test)]
#[path = "click/tests/foreground_gate_187.rs"]
mod foreground_gate_187_tests;

#[cfg(test)]
#[path = "click/tests/foreground_authorization_190.rs"]
mod foreground_authorization_190_tests;

pub use schema::{
    ActClickParams, ActClickPostcondition, ActClickResponse, ActClickTarget, ActClickTierAttempt,
};

const MAX_CLICK_HOLD_MS: u32 = 30_000;
const SUPPORTED_UIA_CLICK_PATTERNS: [&str; 5] = [
    "InvokePattern",
    "TogglePattern",
    "SelectionItemPattern",
    "ExpandCollapsePattern",
    "LegacyIAccessiblePattern.DoDefaultAction",
];
pub(crate) const CLICK_TIER_CDP: &str = "cdp";
pub(crate) const CLICK_TIER_UIA: &str = "uia";
pub(crate) const CLICK_TIER_POSTMESSAGE: &str = "postmessage";
pub(crate) const CLICK_TIER_FOREGROUND: &str = "foreground";
pub(crate) const CLICK_REASON_PATTERN_UNSUPPORTED: &str = "pattern_unsupported";
pub(crate) const CLICK_REASON_ELEMENT_STALE: &str = "element_stale";
pub(crate) const CLICK_REASON_BACKEND_UNAVAILABLE: &str = "backend_unavailable";
pub(crate) const CLICK_REASON_TARGET_INVALID: &str = "target_invalid";
pub(crate) const CLICK_REASON_PARAMS_INVALID: &str = "params_invalid";
pub(crate) const CLICK_REASON_NO_OBSERVED_DELTA: &str = "no_observed_delta";
pub(crate) const CLICK_REASON_SELECTION_ONLY: &str = "selection_only";
pub(crate) const CLICK_REASON_FOREGROUND_REFUSED: &str = "foreground_refused";
pub(crate) const CLICK_REASON_ERROR: &str = "error";
/// #189: the delivery call did not report back, so the input may have been delivered.
pub(crate) const CLICK_REASON_DELIVERY_UNKNOWN: &str = "delivery_unknown";

use crate::server::user_active_gate::UserActiveGate;
use foreground_gate::enforce_foreground_click_auth;
pub(crate) use foreground_gate::{ClaimFacts, ForegroundClickAuth, authorize_foreground_click};

/// Session plus the authorization for the real-foreground (SendInput) tier
/// (#187). The tier is only reachable when `gate` is `Ok`; a denial is carried
/// until a click actually needs SendInput so background tiers keep working.
#[derive(Clone, Debug)]
pub(crate) struct ForegroundClickPolicy {
    session_id: Option<String>,
    gate: Result<ForegroundClickAuth, ErrorData>,
    /// #258: physical user-activity gate, checked right before SendInput.
    user_active: Option<UserActiveGate>,
}

impl Default for ForegroundClickPolicy {
    fn default() -> Self {
        Self::allowed(None)
    }
}

impl ForegroundClickPolicy {
    /// Profile-sanctioned foreground tier (direct callers, recording tests).
    pub(crate) fn allowed(session_id: Option<String>) -> Self {
        Self::authorized(session_id, Ok(ForegroundClickAuth::Profile))
    }

    pub(crate) fn authorized(
        session_id: Option<String>,
        gate: Result<ForegroundClickAuth, ErrorData>,
    ) -> Self {
        Self {
            session_id,
            gate,
            user_active: None,
        }
    }

    /// #258: also refuse the SendInput tier while the human is active.
    pub(crate) fn with_user_active_gate(mut self, user_active: UserActiveGate) -> Self {
        self.user_active = Some(user_active);
        self
    }

    fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Audit label for how the SendInput tier is sanctioned (#190); `None`
    /// when the tier is refused.
    const fn foreground_authorization(&self) -> Option<&'static str> {
        match &self.gate {
            Ok(ForegroundClickAuth::Profile) => Some(FOREGROUND_AUTH_PROFILE),
            Ok(ForegroundClickAuth::Claimed { .. }) => Some(FOREGROUND_AUTH_CLAIMED_TARGET),
            Err(_) => None,
        }
    }

    /// #107: the window this session's claimed route names; `None` for the
    /// profile route or a refused tier.
    const fn claimed_root(&self) -> Option<i64> {
        match &self.gate {
            Ok(ForegroundClickAuth::Claimed { root_hwnd }) => Some(*root_hwnd),
            Ok(ForegroundClickAuth::Profile) | Err(_) => None,
        }
    }

    /// Gate for one SendInput click: the stored authorization, then the live
    /// foreground proof for the claimed route, then the live physical
    /// user-activity check (#258).
    fn authorize_delivery(&self, point: Option<Point>) -> Result<(), ErrorData> {
        match &self.gate {
            Ok(auth) => enforce_foreground_click_auth(*auth, point)?,
            Err(denial) => return Err(denial.clone()),
        }
        self.user_active
            .as_ref()
            .map_or(Ok(()), UserActiveGate::check)
    }

    /// #258 test seam: [`Self::authorize_delivery`] for tests outside `m2::click`
    /// (the policy built by `foreground_click_policy_for_request`).
    #[cfg(test)]
    pub(crate) fn authorize_delivery_for_test(
        &self,
        point: Option<Point>,
    ) -> Result<(), ErrorData> {
        self.authorize_delivery(point)
    }
}

fn point_target_screen_point(target: &ActClickTarget) -> Option<Point> {
    match target {
        ActClickTarget::Point(point) => Some(Point {
            x: point.x,
            y: point.y,
        }),
        ActClickTarget::Element(_) => None,
    }
}

#[allow(dead_code)]
pub async fn act_click_with_handle(
    handle: ActionHandle,
    recording: Option<Arc<RecordingBackend>>,
    params: ActClickParams,
) -> Result<ActClickResponse, ErrorData> {
    act_click_with_handle_and_lease(handle, recording, params, ForegroundClickPolicy::default())
        .await
}

/// `foreground_authorization` value for the #187 claimed-window route.
pub(crate) const FOREGROUND_AUTH_CLAIMED_TARGET: &str = "claimed_target";
/// `foreground_authorization` value for a profile-sanctioned foreground tier.
pub(crate) const FOREGROUND_AUTH_PROFILE: &str = "profile";

pub(crate) async fn act_click_with_handle_and_lease(
    handle: ActionHandle,
    recording: Option<Arc<RecordingBackend>>,
    params: ActClickParams,
    foreground_click_policy: ForegroundClickPolicy,
) -> Result<ActClickResponse, ErrorData> {
    let authorization = foreground_click_policy.foreground_authorization();
    let result = dispatch_click(handle, recording, params, foreground_click_policy).await;
    stamp_foreground_authorization(result, authorization)
}

/// Records on every foreground (SendInput) tier attempt how that tier was
/// sanctioned, on the response and on an error's `data.tier_attempts`, so the
/// audit can tell the claimed-window route from a policy violation (#190).
pub(crate) fn stamp_foreground_authorization(
    result: Result<ActClickResponse, ErrorData>,
    authorization: Option<&'static str>,
) -> Result<ActClickResponse, ErrorData> {
    let Some(authorization) = authorization else {
        return result;
    };
    match result {
        Ok(mut response) => {
            for attempt in &mut response.tier_attempts {
                if attempt.tier == CLICK_TIER_FOREGROUND {
                    attempt.foreground_authorization = Some(authorization.to_owned());
                }
            }
            Ok(response)
        }
        Err(mut error) => {
            if let Some(attempts) = error
                .data
                .as_mut()
                .and_then(|data| data.get_mut("tier_attempts"))
                .and_then(Value::as_array_mut)
            {
                for attempt in attempts.iter_mut().filter(|attempt| {
                    attempt.get("tier").and_then(Value::as_str) == Some(CLICK_TIER_FOREGROUND)
                }) {
                    if let Some(attempt) = attempt.as_object_mut() {
                        attempt.insert(
                            "foreground_authorization".to_owned(),
                            Value::String(authorization.to_owned()),
                        );
                    }
                }
            }
            Err(error)
        }
    }
}

async fn dispatch_click(
    handle: ActionHandle,
    recording: Option<Arc<RecordingBackend>>,
    params: ActClickParams,
    foreground_click_policy: ForegroundClickPolicy,
) -> Result<ActClickResponse, ErrorData> {
    // #189 R5: request validation precedes every dispatching call.
    validate_click_params(&params).map_err(element::mark_click_not_dispatched)?;
    if params.deprecated_curve_alias_used {
        tracing::warn!(
            code = "M2_ACT_CLICK_DEPRECATED_CURVE_ALIAS",
            kind = "act_click",
            replacement = "velocity_profile",
            "act_click deprecated curve alias accepted; use velocity_profile for coordinate-move timing"
        );
    }
    let started = Instant::now();
    let double_click_timing = cached_double_click_timing();
    // #686: a web element id (cdcd sentinel) routes through CDP instead of UIA.
    #[cfg(windows)]
    if let ActClickTarget::Element(element) = &params.target
        && let Some(backend) = synapse_a11y::cdp_backend_from_element_id(&element.element_id)
    {
        ensure_element_transport_backend_allowed(&params, "CDP")
            .map_err(element::mark_click_not_dispatched)?;
        return execute_cdp_click(&params, element, backend, double_click_timing, started).await;
    }
    if let ActClickTarget::Element(element) = &params.target {
        reject_click_modifiers_for_non_cdp(&params, "native/UIA element targets")
            .map_err(element::mark_click_not_dispatched)?;
        ensure_element_transport_backend_allowed(&params, "UIA")
            .map_err(element::mark_click_not_dispatched)?;
        return element::execute_element_click(
            handle,
            &params,
            element,
            recording.as_deref(),
            double_click_timing,
            started,
            foreground_click_policy,
        )
        .await;
    }

    reject_click_modifiers_for_non_cdp(&params, "coordinate point targets")
        .map_err(element::mark_click_not_dispatched)?;
    let target = point_mouse_target(&params.target).map_err(element::mark_click_not_dispatched)?;
    let mut actions = Vec::with_capacity(usize::from(params.clicks) + 1);
    actions.push(Action::MouseMove {
        to: target,
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

    let tier_attempts = if let Some(recording) = recording {
        if let Err(error) =
            record::execute_recording(&recording, &actions, params.clicks, double_click_timing)
                .await
        {
            let error_code = click_error_code(&error);
            let reason_code = click_reason_for_error_code(&error_code);
            let detail = error.message.to_string();
            return Err(attach_click_tier_attempts(
                error,
                vec![click_tier_failed(
                    CLICK_TIER_FOREGROUND,
                    reason_code,
                    error_code,
                    true,
                    detail,
                )],
            ));
        }
        vec![click_tier_delivered(
            CLICK_TIER_FOREGROUND,
            true,
            "screen-coordinate click recorded through the foreground input tier",
        )]
    } else {
        let mut tier_attempts = Vec::new();
        let _lease_guard = acquire_click_foreground_lease(
            &foreground_click_policy,
            params.hold_ms,
            &mut tier_attempts,
            point_target_screen_point(&params.target),
        )?;
        match record::execute_actor_actions(handle, actions, double_click_timing).await {
            Ok(()) => {
                tier_attempts.push(click_tier_delivered(
                    CLICK_TIER_FOREGROUND,
                    true,
                    "screen-coordinate click delivered through the foreground input tier",
                ));
                tier_attempts
            }
            Err(error) => {
                let error_code = click_error_code(&error);
                let reason_code = click_reason_for_error_code(&error_code);
                let detail = error.message.to_string();
                return Err(attach_click_tier_attempts(
                    error,
                    vec![click_tier_failed(
                        CLICK_TIER_FOREGROUND,
                        reason_code,
                        error_code,
                        true,
                        detail,
                    )],
                ));
            }
        }
    };
    let backend_tier_used = click_backend_tier_used(&tier_attempts);
    let required_foreground = click_required_foreground(&tier_attempts);

    Ok(ActClickResponse {
        ok: true,
        used_invoke_pattern: false,
        backend_used: backend_used_name(params.backend).to_owned(),
        backend_tier_used,
        required_foreground,
        tier_attempts,
        postcondition: schema::postcondition_not_requested(),
        press_hold_ms: params.hold_ms,
        double_click_window_ms: double_click_timing.window_ms,
        inter_click_delay_ms: double_click_timing.inter_click_delay_ms,
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}

/// Routes a click on a CDP web element id through CDP (#686): resolve the
/// browser's debug endpoint from the element's window, scroll the node into
/// view, and dispatch the click in viewport coordinates. Fail-loud if the
/// endpoint is gone or the node cannot be resolved.
#[cfg(windows)]
async fn execute_cdp_click(
    params: &ActClickParams,
    element: &schema::ActClickElementTarget,
    backend_node_id: i64,
    double_click_timing: synapse_action::DoubleClickTiming,
    started: Instant,
) -> Result<ActClickResponse, ErrorData> {
    use synapse_core::MouseButton;

    let hwnd = element
        .element_id
        .parts()
        .map_err(|err| {
            let detail = format!("web element id is malformed: {err}");
            attach_click_tier_attempts(
                element::mark_click_not_dispatched(mcp_error(
                    error_codes::ACTION_ELEMENT_NOT_RESOLVED,
                    detail.clone(),
                )),
                vec![click_tier_failed(
                    CLICK_TIER_CDP,
                    CLICK_REASON_TARGET_INVALID,
                    error_codes::ACTION_ELEMENT_NOT_RESOLVED,
                    false,
                    detail,
                )],
            )
        })?
        .hwnd;
    // Foreground window title disambiguates which tab owns the per-document node.
    let title_hint = synapse_a11y::foreground_context(hwnd)
        .map(|context| context.window_title)
        .unwrap_or_default();
    let target_id_hint = synapse_a11y::cdp_target_from_element_id(&element.element_id);
    let button = match params.button {
        MouseButton::Left => synapse_a11y::CdpMouseButton::Left,
        MouseButton::Right => synapse_a11y::CdpMouseButton::Right,
        MouseButton::Middle => synapse_a11y::CdpMouseButton::Middle,
        other => {
            let detail =
                format!("act_click button {other:?} is not supported for web (CDP) elements");
            return Err(attach_click_tier_attempts(
                element::mark_click_not_dispatched(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    detail.clone(),
                )),
                vec![click_tier_failed(
                    CLICK_TIER_CDP,
                    CLICK_REASON_PARAMS_INVALID,
                    error_codes::TOOL_PARAMS_INVALID,
                    false,
                    detail,
                )],
            ));
        }
    };
    let modifiers = cdp_click_modifier_bits(&params.modifiers);

    remote_click_with(async {
        if let Some(endpoint) = synapse_a11y::endpoint_for_window(hwnd) {
            synapse_a11y::cdp_click_node(
                &endpoint,
                &title_hint,
                target_id_hint.as_deref(),
                backend_node_id,
                button,
                i64::from(params.clicks),
                modifiers,
            )
            .await
            .map_err(|err| {
                let error = remote_cdp_click_failure(
                    &err,
                    action_error_to_mcp(&a11y_to_action_error(&err)),
                );
                attach_click_tier_attempts(
                    error,
                    vec![click_tier_failed(
                        CLICK_TIER_CDP,
                        click_reason_for_error_code(err.code()),
                        err.code(),
                        false,
                        err.to_string(),
                    )],
                )
            })?;
        } else {
            let bridge_button = match button {
                synapse_a11y::CdpMouseButton::Left => {
                    crate::chrome_debugger_bridge::ChromeDebuggerMouseButton::Left
                }
                synapse_a11y::CdpMouseButton::Right => {
                    crate::chrome_debugger_bridge::ChromeDebuggerMouseButton::Right
                }
                synapse_a11y::CdpMouseButton::Middle => {
                    crate::chrome_debugger_bridge::ChromeDebuggerMouseButton::Middle
                }
            };
            crate::chrome_debugger_bridge::click_node(
                hwnd,
                &title_hint,
                target_id_hint.as_deref(),
                backend_node_id,
                bridge_button,
                i64::from(params.clicks),
            )
            .await
            .map_err(|err| {
                let detail = format!("Chrome debugger extension click failed: {}", err.detail());
                attach_click_tier_attempts(
                    remote_delivery::stamp_bridge_dispatch_stage(
                        mcp_error(err.code(), detail.clone()),
                        err.dispatch_stage(),
                    ),
                    vec![click_tier_failed(
                        CLICK_TIER_CDP,
                        click_reason_for_error_code(err.code()),
                        err.code(),
                        false,
                        detail,
                    )],
                )
            })?;
        }

        Ok(())
    })
    .await?;

    Ok(ActClickResponse {
        ok: true,
        used_invoke_pattern: false,
        backend_used: "cdp".to_owned(),
        backend_tier_used: CLICK_TIER_CDP.to_owned(),
        required_foreground: false,
        tier_attempts: vec![click_tier_delivered(
            CLICK_TIER_CDP,
            false,
            "web element click delivered through Chrome DevTools Protocol",
        )],
        postcondition: schema::postcondition_not_requested(),
        press_hold_ms: params.hold_ms,
        double_click_window_ms: double_click_timing.window_ms,
        inter_click_delay_ms: double_click_timing.inter_click_delay_ms,
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}

/// Maps an a11y CDP error to an action error so it surfaces with the same shape
/// as other action failures.
#[cfg(windows)]
fn a11y_to_action_error(err: &synapse_a11y::A11yError) -> ActionError {
    ActionError::TargetInvalid {
        detail: format!("{} ({})", err, err.code()),
    }
}

pub(crate) fn click_tier_delivered(
    tier: impl Into<String>,
    required_foreground: bool,
    detail: impl Into<String>,
) -> ActClickTierAttempt {
    let attempt = ActClickTierAttempt {
        tier: tier.into(),
        status: "delivered".to_owned(),
        reason_code: None,
        error_code: None,
        detail: Some(detail.into()),
        required_foreground,
        foreground_authorization: None,
    };
    log_click_tier_attempt(&attempt);
    attempt
}

pub(crate) fn click_tier_failed(
    tier: impl Into<String>,
    reason_code: impl Into<String>,
    error_code: impl Into<String>,
    required_foreground: bool,
    detail: impl Into<String>,
) -> ActClickTierAttempt {
    let attempt = ActClickTierAttempt {
        tier: tier.into(),
        status: "failed".to_owned(),
        reason_code: Some(reason_code.into()),
        error_code: Some(error_code.into()),
        detail: Some(detail.into()),
        required_foreground,
        foreground_authorization: None,
    };
    log_click_tier_attempt(&attempt);
    attempt
}

pub(crate) fn attach_click_tier_attempts(
    mut error: ErrorData,
    mut tier_attempts: Vec<ActClickTierAttempt>,
) -> ErrorData {
    if error
        .data
        .as_ref()
        .is_some_and(|data| data["delivered"] == true)
        && !tier_attempts
            .iter()
            .any(|attempt| attempt.status == "delivered")
        && let Some(failed) = tier_attempts.last().cloned()
    {
        let delivered = click_tier_delivered(
            failed.tier,
            failed.required_foreground,
            "input delivered before this tier reported failure",
        );
        tier_attempts.insert(tier_attempts.len().saturating_sub(1), delivered);
    }
    let attempts = serde_json::to_value(&tier_attempts).unwrap_or_else(|err| {
        json!([{
            "tier": "telemetry",
            "status": "failed",
            "reason_code": "attempt_chain_encode_failed",
            "error_code": error_codes::TOOL_INTERNAL_ERROR,
            "detail": err.to_string(),
            "required_foreground": false,
        }])
    });
    let mut data = match error.data.take() {
        Some(Value::Object(map)) => map,
        Some(other) => {
            let mut map = Map::new();
            map.insert("original_data".to_owned(), other);
            map
        }
        None => Map::new(),
    };
    data.insert("tier_attempts".to_owned(), attempts);
    data.insert("silent_fallback_allowed".to_owned(), Value::Bool(false));
    error.data = Some(Value::Object(data));
    error
}

pub(crate) fn click_backend_tier_used(tier_attempts: &[ActClickTierAttempt]) -> String {
    tier_attempts
        .iter()
        .rev()
        .find(|attempt| attempt.status == "delivered")
        .map(|attempt| attempt.tier.clone())
        .unwrap_or_else(|| "none".to_owned())
}

pub(crate) fn click_required_foreground(tier_attempts: &[ActClickTierAttempt]) -> bool {
    tier_attempts
        .iter()
        .rev()
        .find(|attempt| attempt.status == "delivered")
        .is_some_and(|attempt| attempt.required_foreground)
}

pub(crate) fn click_params_can_route_background_first(params: &ActClickParams) -> bool {
    if !matches!(params.backend, Backend::Auto | Backend::Software) {
        return false;
    }
    match &params.target {
        ActClickTarget::Element(element) => {
            #[cfg(windows)]
            if synapse_a11y::cdp_backend_from_element_id(&element.element_id).is_some() {
                return false;
            }
            !params.use_invoke_pattern || params.coordinate_fallback_on_unsupported
        }
        ActClickTarget::Point(_) => false,
    }
}

pub(crate) fn click_target_root_hwnd(params: &ActClickParams) -> Result<Option<i64>, ErrorData> {
    match &params.target {
        ActClickTarget::Element(element) => {
            let parsed_hwnd = element
                .element_id
                .parts()
                .map_err(|error| {
                    mcp_error(
                        error_codes::ACTION_TARGET_INVALID,
                        format!(
                            "act_click element id {} could not be parsed for target-window verification: {error}",
                            element.element_id
                        ),
                    )
                })?
                .hwnd;
            let hwnd = crate::m2::live_target_root(
                parsed_hwnd,
                &format!(
                    "act_click element id {} target window check",
                    element.element_id
                ),
            )?;
            if let Some(expected) = params.verify_target_window_hwnd {
                let expected_root = crate::m2::live_target_root(
                    expected,
                    "act_click verify_target_window_hwnd check for element target",
                )?;
                if expected_root != hwnd {
                    return Err(mcp_error(
                        error_codes::ACTION_TARGET_INVALID,
                        format!(
                            "act_click verify_target_window_hwnd 0x{expected_root:x} does not match element target root HWND 0x{hwnd:x}"
                        ),
                    ));
                }
            }
            Ok(Some(hwnd))
        }
        ActClickTarget::Point(_) => params
            .verify_target_window_hwnd
            .map(|hwnd| {
                crate::m2::live_target_root(
                    hwnd,
                    "act_click verify_target_window_hwnd check for point target",
                )
            })
            .transpose(),
    }
}

pub(crate) fn click_target_foreground_guard_hwnds(
    params: &ActClickParams,
) -> Result<Option<(i64, i64)>, ErrorData> {
    match &params.target {
        ActClickTarget::Element(element) => {
            let element_hwnd = element
                .element_id
                .parts()
                .map_err(|error| {
                    mcp_error(
                        error_codes::ACTION_TARGET_INVALID,
                        format!(
                            "act_click element id {} could not be parsed for foreground guard: {error}",
                            element.element_id
                        ),
                    )
                })?
                .hwnd;
            let root_hwnd = crate::m2::live_target_root(
                element_hwnd,
                &format!(
                    "act_click element id {} target window check before delivery",
                    element.element_id
                ),
            )?;
            Ok(Some((element_hwnd, root_hwnd)))
        }
        // #107: a point click that names its window runs under the same
        // foreground guard as element clicks (settle, restore, report).
        ActClickTarget::Point(_) => params
            .verify_target_window_hwnd
            .map(|hwnd| {
                crate::m2::live_target_root(
                    hwnd,
                    "act_click verify_target_window_hwnd check before delivery",
                )
                .map(|root_hwnd| (root_hwnd, root_hwnd))
            })
            .transpose(),
    }
}

pub(crate) fn click_error_code(error: &ErrorData) -> String {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned()
}

pub(crate) fn click_reason_for_error_code(error_code: &str) -> &'static str {
    match error_code {
        error_codes::FOREGROUND_ACTIVATION_REFUSED | error_codes::TOOL_PROFILE_POLICY_DENIED => {
            CLICK_REASON_FOREGROUND_REFUSED
        }
        error_codes::ACTION_FOREGROUND_LEASE_BUSY => "foreground_lease_busy",
        error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED => CLICK_REASON_PATTERN_UNSUPPORTED,
        error_codes::TRANSIENT_ELEMENT_EXPIRED | error_codes::A11Y_ELEMENT_STALE => {
            CLICK_REASON_ELEMENT_STALE
        }
        error_codes::ACTION_BACKEND_UNAVAILABLE
        | error_codes::ACTION_QUEUE_FULL
        | error_codes::ACTION_RATE_LIMITED
        | error_codes::A11Y_CDP_UNREACHABLE
        | error_codes::A11Y_CDP_ATTACH_FAILED
        | error_codes::A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED
        | error_codes::A11Y_CDP_AXTREE_FAILED => CLICK_REASON_BACKEND_UNAVAILABLE,
        error_codes::ACTION_TARGET_INVALID | error_codes::ACTION_ELEMENT_NOT_RESOLVED => {
            CLICK_REASON_TARGET_INVALID
        }
        error_codes::TOOL_PARAMS_INVALID => CLICK_REASON_PARAMS_INVALID,
        error_codes::ACTION_NO_OBSERVED_DELTA => CLICK_REASON_NO_OBSERVED_DELTA,
        _ => CLICK_REASON_ERROR,
    }
}

pub(super) fn acquire_click_foreground_lease(
    foreground_click_policy: &ForegroundClickPolicy,
    hold_ms: u32,
    tier_attempts: &mut Vec<ActClickTierAttempt>,
    click_point: Option<Point>,
) -> Result<crate::m2::ForegroundInputLeaseGuard, ErrorData> {
    if let Err(error) = foreground_click_policy.authorize_delivery(click_point) {
        let error_code = click_error_data_code(&error)
            .unwrap_or(error_codes::TOOL_PROFILE_POLICY_DENIED)
            .to_owned();
        tier_attempts.push(click_tier_failed(
            CLICK_TIER_FOREGROUND,
            click_reason_for_error_code(&error_code),
            error_code,
            true,
            error.message.to_string(),
        ));
        // #189: a refused SendInput tier sent no input.
        return Err(attach_click_tier_attempts(
            element::mark_click_not_dispatched(error),
            tier_attempts.clone(),
        ));
    }
    match crate::m2::acquire_foreground_input_lease_with_ttl(
        "act_click",
        foreground_click_policy.session_id(),
        crate::m2::foreground_input_lease_ttl_for_hold_ms(hold_ms),
    ) {
        Ok(guard) => Ok(guard),
        Err(error) => {
            let error_code = click_error_data_code(&error)
                .unwrap_or(error_codes::ACTION_FOREGROUND_LEASE_BUSY)
                .to_owned();
            tier_attempts.push(click_tier_failed(
                CLICK_TIER_FOREGROUND,
                click_reason_for_error_code(&error_code),
                error_code,
                true,
                error.message.to_string(),
            ));
            Err(attach_click_tier_attempts(
                element::mark_click_not_dispatched(error),
                tier_attempts.clone(),
            ))
        }
    }
}

fn click_error_data_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn log_click_tier_attempt(attempt: &ActClickTierAttempt) {
    let tier = attempt.tier.as_str();
    let status = attempt.status.as_str();
    let reason_code = attempt.reason_code.as_deref().unwrap_or("none");
    let error_code = attempt.error_code.as_deref().unwrap_or("none");
    let detail = attempt.detail.as_deref().unwrap_or("");
    if status == "failed" {
        tracing::warn!(
            code = "M2_ACT_CLICK_TIER_ATTEMPT",
            kind = "act_click",
            tier,
            status,
            reason_code,
            error_code,
            required_foreground = attempt.required_foreground,
            detail,
            "act_click backend tier attempt failed"
        );
    } else {
        tracing::info!(
            code = "M2_ACT_CLICK_TIER_ATTEMPT",
            kind = "act_click",
            tier,
            status,
            reason_code,
            error_code,
            required_foreground = attempt.required_foreground,
            detail,
            "act_click backend tier attempt delivered"
        );
    }
}

fn validate_click_params(params: &ActClickParams) -> Result<(), ErrorData> {
    if !(1..=3).contains(&params.clicks) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_click clicks must be in 1..=3, got {}", params.clicks),
        ));
    }
    if params.hold_ms == 0 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "act_click hold_ms must be at least 1",
        ));
    }
    if params.hold_ms > MAX_CLICK_HOLD_MS {
        return Err(action_error_to_mcp(&ActionError::HoldExceededMax {
            detail: format!(
                "act_click hold_ms {} exceeds max {MAX_CLICK_HOLD_MS}",
                params.hold_ms
            ),
        }));
    }
    if !(50..=5000).contains(&params.verify_timeout_ms) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "act_click verify_timeout_ms must be in 50..=5000, got {}",
                params.verify_timeout_ms
            ),
        ));
    }
    Ok(())
}

fn cdp_click_modifier_bits(modifiers: &[schema::ClickModifier]) -> i64 {
    modifiers.iter().fold(0_i64, |bits, modifier| {
        bits | match modifier {
            schema::ClickModifier::Alt => 1,
            schema::ClickModifier::Ctrl => 2,
            schema::ClickModifier::Super => 4,
            schema::ClickModifier::Shift => 8,
        }
    })
}

fn reject_click_modifiers_for_non_cdp(
    params: &ActClickParams,
    target_kind: &str,
) -> Result<(), ErrorData> {
    if params.modifiers.is_empty() {
        return Ok(());
    }
    Err(action_error_to_mcp(&ActionError::BackendUnavailable {
        detail: format!(
            "act_click modifiers are supported only for web (CDP) element targets, not {target_kind}"
        ),
    }))
}

fn ensure_element_transport_backend_allowed(
    params: &ActClickParams,
    transport: &str,
) -> Result<(), ErrorData> {
    if matches!(params.backend, Backend::Vigem | Backend::Hardware) {
        let tier = match transport {
            "CDP" => CLICK_TIER_CDP,
            "PostMessage" => CLICK_TIER_POSTMESSAGE,
            _ => CLICK_TIER_UIA,
        };
        let detail = format!(
            "act_click element target requested backend={} but {transport} element delivery is only valid for backend=auto or backend=software; no fallback delivery was attempted",
            backend_used_name(params.backend)
        );
        let error = action_error_to_mcp(&ActionError::BackendUnavailable {
            detail: detail.clone(),
        });
        return Err(attach_click_tier_attempts(
            error,
            vec![click_tier_failed(
                tier,
                CLICK_REASON_BACKEND_UNAVAILABLE,
                error_codes::ACTION_BACKEND_UNAVAILABLE,
                false,
                detail,
            )],
        ));
    }
    Ok(())
}

fn point_mouse_target(target: &ActClickTarget) -> Result<MouseTarget, ErrorData> {
    match target {
        ActClickTarget::Point(point) => Ok(MouseTarget::Screen {
            point: Point {
                x: point.x,
                y: point.y,
            },
        }),
        ActClickTarget::Element(element) => {
            let detail = format!(
                "act_click element target {} reached the point-target path unexpectedly",
                element.element_id
            );
            let error = action_error_to_mcp(&ActionError::TargetInvalid {
                detail: format!(
                    "act_click element target {} reached the point-target path unexpectedly",
                    element.element_id
                ),
            });
            Err(attach_click_tier_attempts(
                error,
                vec![click_tier_failed(
                    CLICK_TIER_FOREGROUND,
                    CLICK_REASON_TARGET_INVALID,
                    error_codes::ACTION_TARGET_INVALID,
                    true,
                    detail,
                )],
            ))
        }
    }
}

fn action_error_to_mcp(error: &ActionError) -> ErrorData {
    match error {
        ActionError::TransientElementExpired { element_id, detail } => {
            transient_element_expired_error(element_id, detail)
        }
        ActionError::ElementPatternUnsupported { element_id, detail } => {
            element_pattern_unsupported_error(element_id, detail)
        }
        ActionError::ClickNotDispatched { source, .. } => {
            element::mark_click_not_dispatched(action_error_to_mcp(source))
        }
        _ => crate::m2::action_error_to_mcp(error),
    }
}

fn transient_element_expired_error(element_id: &ElementId, detail: &str) -> ErrorData {
    let root_hwnd = element_id.parts().ok().map(|parts| parts.hwnd);
    let recommended_pattern = "Call observe or find again immediately before acting on the transient UI, then pass the fresh element_id to act_click; do not reuse element_ids from expired toast/snackbar observations.";
    let tier_attempts = vec![click_tier_failed(
        CLICK_TIER_UIA,
        CLICK_REASON_ELEMENT_STALE,
        error_codes::TRANSIENT_ELEMENT_EXPIRED,
        false,
        detail.to_owned(),
    )];
    tracing::warn!(
        code = error_codes::TRANSIENT_ELEMENT_EXPIRED,
        element_id = %element_id,
        root_hwnd,
        detail,
        recommended_pattern,
        "act_click transient UI element expired before dispatch; no fallback click attempted"
    );
    attach_click_tier_attempts(
        ErrorData::new(
            ErrorCode(-32099),
            format!("transient UI element expired before act_click dispatch: {detail}"),
            Some(json!({
                "code": error_codes::TRANSIENT_ELEMENT_EXPIRED,
                "detail_code": "UIA_ELEMENT_STALE_AFTER_OBSERVE",
                "transient": true,
                "fallback_attempted": false,
                "element_id": element_id.to_string(),
                "root_hwnd": root_hwnd,
                "source_of_truth": "live UI Automation re-resolution under the element_id root HWND",
                "recommended_next_tools": ["observe", "find", "act_click"],
                "recommended_pattern": recommended_pattern,
                "detail": detail,
            })),
        ),
        tier_attempts,
    )
}

fn element_pattern_unsupported_error(element_id: &ElementId, detail: &str) -> ErrorData {
    let root_hwnd = element_id.parts().ok().map(|parts| parts.hwnd);
    let tier_attempts = vec![click_tier_failed(
        CLICK_TIER_UIA,
        CLICK_REASON_PATTERN_UNSUPPORTED,
        error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED,
        false,
        detail.to_owned(),
    )];
    tracing::warn!(
        code = error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED,
        element_id = %element_id,
        root_hwnd,
        attempted_patterns = ?SUPPORTED_UIA_CLICK_PATTERNS,
        detail,
        fallback_attempted = false,
        "act_click element target exposes no supported UIA click control pattern; no fallback delivery attempted"
    );
    attach_click_tier_attempts(
        ErrorData::new(
            ErrorCode(-32099),
            format!("element target exposes no supported UIA click control pattern: {detail}"),
            Some(json!({
                "code": error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED,
                "detail_code": "UIA_CONTROL_PATTERN_UNSUPPORTED",
                "transient": false,
                "fallback_attempted": false,
                "element_id": element_id.to_string(),
                "root_hwnd": root_hwnd,
                "attempted_patterns": SUPPORTED_UIA_CLICK_PATTERNS,
                "source_of_truth": "live UI Automation control-pattern availability on the re-resolved element",
                "dispatch_not_started": true,
                "router_escalation_required": true,
                "router_next_tier": "postmessage",
                "detail": detail,
            })),
        ),
        tier_attempts,
    )
}

const fn backend_used_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Auto | Backend::Software => "software",
        Backend::Vigem => "vigem",
        Backend::Hardware => "hardware",
    }
}
