use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use rmcp::ErrorData;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use synapse_action::{ActionError, ActionHandle, EmitState, RecordedInput, RecordingBackend};
use synapse_core::{Action, Backend, ElementId, Point, error_codes};

use crate::m1::mcp_error;
use crate::m2::postcondition::{
    ActPostcondition, default_verify_timeout_ms, postcondition_not_requested,
};
#[cfg(windows)]
use crate::m2::postcondition::{hash_json, no_observed_delta_error, postcondition_observed_delta};
#[cfg(windows)]
use serde_json::{Value, json};

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use synapse_action::backend::software::{cursor_position, ensure_pointer_foreground};
#[cfg(windows)]
use synapse_action::delivery_target::{
    INPUT_TARGET_REMEDIATION, InputDeliveryTarget, wheel_delivery_point,
};
#[cfg(windows)]
use synapse_core::Rect;
#[cfg(windows)]
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT as WinPoint, RECT, WPARAM},
    UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetClassNameW, GetWindowRect, IsWindowVisible, PostMessageW,
        WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WindowFromPoint,
    },
};

const SMOOTH_SCROLL_INTERVAL_MS: u32 = 30;
const MAX_SMOOTH_SCROLL_STEPS: u32 = 120;
const WHEEL_DELTA: i32 = 120;
#[cfg(windows)]
const MAX_TARGETED_WHEEL_MESSAGES: usize = 1024;
#[cfg(windows)]
const SOURCE_UIA_SCROLL_PATTERN: &str = "uia_scroll_pattern.scroll_state";
#[cfg(windows)]
const SOURCE_UIA_SCROLL_ITEM: &str = "uia_scroll_item_pattern.bounding_rect";

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActScrollParams {
    #[serde(default)]
    #[schemars(default)]
    pub dy: i32,
    #[serde(default)]
    #[schemars(default)]
    pub dx: i32,
    pub at: Option<ActScrollPoint>,
    pub target: Option<ActScrollElementTarget>,
    #[serde(default)]
    #[schemars(default)]
    pub smooth: bool,
    #[serde(default)]
    #[schemars(default)]
    pub verify_delta: bool,
    #[serde(default = "default_verify_timeout_ms")]
    #[schemars(default = "default_verify_timeout_ms", range(min = 50, max = 5000))]
    pub verify_timeout_ms: u32,
}

#[derive(Copy, Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActScrollPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActScrollElementTarget {
    pub element_id: ElementId,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActScrollResponse {
    pub ok: bool,
    pub dy: i32,
    pub dx: i32,
    pub smooth: bool,
    pub scrolled: bool,
    pub wheel_event_count: u32,
    pub smooth_interval_ms: u32,
    pub scheduled_smooth_total_ms: u32,
    pub backend_used: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
    pub elapsed_ms: u32,
    pub postcondition: ActPostcondition,
}

pub(crate) async fn act_scroll_with_handle_bound_target(
    handle: ActionHandle,
    recording: Option<Arc<RecordingBackend>>,
    params: ActScrollParams,
    checked_window_hwnd: Option<i64>,
) -> Result<ActScrollResponse, ErrorData> {
    validate_scroll_params(&params)?;
    let started = Instant::now();
    if params.dy == 0 && params.dx == 0 {
        if let Some(recording) = recording {
            execute_recording_noop(&recording, &params);
        }
        // Nothing to scroll: no step runs, so the reply is not smooth (#2401 P4).
        return Ok(response(
            &params,
            false,
            0,
            "none",
            ScrollPacing::new(params.smooth, true, params.dy, params.dx),
            started,
        ));
    }

    if let Some(target) = &params.target {
        #[cfg(windows)]
        {
            if let Some(backend_node_id) =
                synapse_a11y::cdp_backend_from_element_id(&target.element_id)
            {
                return execute_cdp_scroll(&params, &target.element_id, backend_node_id, started)
                    .await;
            }
            return execute_uia_scroll(&params, &target.element_id, started).await;
        }
        #[cfg(not(windows))]
        {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "act_scroll target.element_id {} is not a CDP web element id; UIA element-target scroll requires Windows ScrollPattern or ScrollItemPattern",
                    target.element_id
                ),
            ));
        }
    }

    let (wheel_event_count, backend_used, pacing) = if let Some(recording) = recording {
        let actions = scroll_actions(&params)?;
        execute_recording(&recording, &actions, &params).await?;
        (
            actions.len(),
            "software",
            ScrollPacing::new(params.smooth, true, params.dy, params.dx),
        )
    } else {
        let dispatch = execute_native_scroll(&handle, &params, checked_window_hwnd).await?;
        (
            dispatch.wheel_event_count,
            dispatch.backend_used,
            dispatch.pacing,
        )
    };

    Ok(response(
        &params,
        true,
        wheel_event_count,
        backend_used,
        pacing,
        started,
    ))
}

impl ActScrollParams {
    pub(crate) const fn requires_input_lease(&self) -> bool {
        self.target.is_none() && self.at.is_none() && (self.dy != 0 || self.dx != 0)
    }

    pub(crate) fn verify_delta_point_region(&self) -> Option<Point> {
        if self.target.is_some() {
            None
        } else {
            self.at.map(Into::into)
        }
    }

    pub(crate) fn uses_element_target(&self) -> bool {
        self.target.is_some()
    }
}

impl From<ActScrollPoint> for Point {
    fn from(value: ActScrollPoint) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

fn validate_scroll_params(params: &ActScrollParams) -> Result<(), ErrorData> {
    if params.at.is_some() && params.target.is_some() {
        return Err(mcp_error(
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "act_scroll accepts either at={x,y} or target={element_id}, not both",
        ));
    }
    if params.smooth {
        let step_count = smooth_step_count(params.dy, params.dx);
        if step_count > MAX_SMOOTH_SCROLL_STEPS {
            return Err(mcp_error(
                synapse_core::error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "act_scroll smooth=true step count {step_count} exceeds max {MAX_SMOOTH_SCROLL_STEPS}"
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
async fn execute_cdp_scroll(
    params: &ActScrollParams,
    element_id: &ElementId,
    backend_node_id: i64,
    started: Instant,
) -> Result<ActScrollResponse, ErrorData> {
    let hwnd = element_id
        .parts()
        .map_err(|err| {
            mcp_error(
                error_codes::ACTION_ELEMENT_NOT_RESOLVED,
                format!("web element id is malformed: {err}"),
            )
        })?
        .hwnd;
    let endpoint = synapse_a11y::endpoint_for_window(hwnd).ok_or_else(|| {
        mcp_error(
            error_codes::A11Y_CDP_UNREACHABLE,
            format!(
                "no reachable CDP endpoint for web element {element_id} (browser closed or debug port gone)"
            ),
        )
    })?;
    let title_hint = synapse_a11y::foreground_context(hwnd)
        .map(|context| context.window_title)
        .unwrap_or_default();
    let target_id_hint = synapse_a11y::cdp_target_from_element_id(element_id);
    let before = if params.verify_delta {
        Some(
            synapse_a11y::cdp_node_scroll_state(
                &endpoint,
                &title_hint,
                target_id_hint.as_deref(),
                backend_node_id,
            )
            .await
            .map_err(|err| mcp_error(err.code(), err.to_string()))?,
        )
    } else {
        None
    };
    let deltas = cdp_wheel_deltas(params)?;
    let wheel_event_count = deltas.len();
    let point = synapse_a11y::cdp_scroll_node(
        &endpoint,
        &title_hint,
        target_id_hint.as_deref(),
        backend_node_id,
        deltas,
        if params.smooth {
            SMOOTH_SCROLL_INTERVAL_MS
        } else {
            0
        },
    )
    .await
    .map_err(|err| mcp_error(err.code(), err.to_string()))?;
    let postcondition = if let Some(before) = before {
        tokio::time::sleep(Duration::from_millis(u64::from(params.verify_timeout_ms))).await;
        let after = synapse_a11y::cdp_node_scroll_state(
            &endpoint,
            &title_hint,
            target_id_hint.as_deref(),
            backend_node_id,
        )
        .await
        .map_err(|err| mcp_error(err.code(), err.to_string()))?;
        let postcondition = verify_cdp_scroll_delta(params.verify_timeout_ms, &before, &after)?;
        tracing::info!(
            code = "M2_ACT_SCROLL_CDP_WHEEL",
            kind = "act_scroll",
            element_id = %element_id,
            backend_node_id,
            viewport_x = point.x,
            viewport_y = point.y,
            wheel_event_count,
            dy = params.dy,
            dx = params.dx,
            smooth = params.smooth,
            before = ?before,
            after = ?after,
            "readback=cdp_node.scroll_state tool=act_scroll cdp_scroll_after"
        );
        postcondition
    } else {
        tracing::info!(
            code = "M2_ACT_SCROLL_CDP_WHEEL",
            kind = "act_scroll",
            element_id = %element_id,
            backend_node_id,
            viewport_x = point.x,
            viewport_y = point.y,
            wheel_event_count,
            dy = params.dy,
            dx = params.dx,
            smooth = params.smooth,
            "readback=cdp_dispatch tool=act_scroll cdp_scroll_after"
        );
        postcondition_not_requested("act_scroll", "cdp_node.scroll_state")
    };
    let pacing = ScrollPacing::new(params.smooth, true, params.dy, params.dx);
    let mut response = response(params, true, wheel_event_count, "cdp", pacing, started);
    response.postcondition = postcondition;
    Ok(response)
}

#[cfg(windows)]
async fn execute_uia_scroll(
    params: &ActScrollParams,
    element_id: &ElementId,
    started: Instant,
) -> Result<ActScrollResponse, ErrorData> {
    let mut readback =
        synapse_a11y::scroll_element(element_id, params.dy, params.dx).map_err(|error| {
            mcp_error(
                error.code(),
                format!("act_scroll UIA target scroll failed for element {element_id}: {error}"),
            )
        })?;
    if params.verify_delta {
        tokio::time::sleep(Duration::from_millis(u64::from(params.verify_timeout_ms))).await;
        readback.after = synapse_a11y::element_scroll_state(element_id).map_err(|error| {
            mcp_error(
                error.code(),
                format!(
                    "act_scroll UIA Source-of-Truth readback failed for element {element_id}: {error}"
                ),
            )
        })?;
    }

    let postcondition = uia_scroll_postcondition(params, &readback)?;
    let backend_used = uia_scroll_backend_used(&readback);
    tracing::info!(
        code = "M2_ACT_SCROLL_UIA_PATTERN",
        kind = "act_scroll",
        element_id = %element_id,
        method = %readback.method,
        backend_used,
        dy = params.dy,
        dx = params.dx,
        scroll_call_count = readback.scroll_call_count,
        verify_delta = params.verify_delta,
        before = ?readback.before,
        after = ?readback.after,
        "readback=uia_scroll_state tool=act_scroll uia_scroll_after"
    );

    // The UIA pattern scrolls in one call and never pauses (#2401).
    let mut response = response(
        params,
        true,
        usize::try_from(readback.scroll_call_count).unwrap_or(usize::MAX),
        backend_used,
        ScrollPacing::new(params.smooth, false, params.dy, params.dx),
        started,
    );
    response.postcondition = postcondition;
    Ok(response)
}

#[cfg(windows)]
fn uia_scroll_postcondition(
    params: &ActScrollParams,
    readback: &synapse_a11y::ElementScrollReadback,
) -> Result<ActPostcondition, ErrorData> {
    let source_of_truth = uia_scroll_source_of_truth(readback);
    if !params.verify_delta {
        return Ok(postcondition_not_requested("act_scroll", source_of_truth));
    }
    let before_signature = hash_json(&readback.before)?;
    let after_signature = hash_json(&readback.after)?;
    if readback.before == readback.after {
        return Err(no_observed_delta_error(
            "act_scroll",
            source_of_truth,
            params.verify_timeout_ms,
            before_signature,
            after_signature,
            json!({
                "method": readback.method,
                "requested_dy": readback.requested_dy,
                "requested_dx": readback.requested_dx,
                "scroll_call_count": readback.scroll_call_count,
                "before": &readback.before,
                "after": &readback.after,
            }),
        ));
    }
    Ok(postcondition_observed_delta(
        "act_scroll",
        source_of_truth,
        before_signature,
        after_signature,
        "observed UIA target scroll state change after control-pattern dispatch",
    ))
}

#[cfg(windows)]
fn uia_scroll_backend_used(readback: &synapse_a11y::ElementScrollReadback) -> &'static str {
    if readback.method == "uia_scroll_item_pattern" {
        "uia_scroll_item_pattern"
    } else {
        "uia_scroll_pattern"
    }
}

#[cfg(windows)]
fn uia_scroll_source_of_truth(readback: &synapse_a11y::ElementScrollReadback) -> &'static str {
    if readback.method == "uia_scroll_item_pattern" {
        SOURCE_UIA_SCROLL_ITEM
    } else {
        SOURCE_UIA_SCROLL_PATTERN
    }
}

#[cfg(windows)]
fn cdp_wheel_deltas(
    params: &ActScrollParams,
) -> Result<Vec<synapse_a11y::CdpWheelDelta>, ErrorData> {
    let mut ticks = if params.smooth {
        smooth_scroll_ticks(params)?
    } else {
        vec![(params.dy, params.dx)]
    };
    ticks.retain(|(dy, dx)| *dy != 0 || *dx != 0);
    Ok(ticks
        .into_iter()
        .map(|(dy, dx)| synapse_a11y::CdpWheelDelta {
            delta_x: cdp_delta_x_for_ticks(dx),
            delta_y: cdp_delta_y_for_ticks(dy),
        })
        .collect())
}

#[cfg(windows)]
fn smooth_scroll_ticks(params: &ActScrollParams) -> Result<Vec<(i32, i32)>, ErrorData> {
    let step_count = smooth_step_count(params.dy, params.dx);
    let capacity = usize::try_from(step_count).map_err(|_err| {
        mcp_error(
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "act_scroll smooth=true step count cannot fit in memory",
        )
    })?;
    let mut ticks = Vec::with_capacity(capacity);
    let mut vertical_ticks_remaining = params.dy;
    let mut horizontal_ticks_remaining = params.dx;
    for _ in 0..step_count {
        ticks.push((
            take_tick(&mut vertical_ticks_remaining),
            take_tick(&mut horizontal_ticks_remaining),
        ));
    }
    Ok(ticks)
}

/// #192: dx > 0 scrolls RIGHT on every route (Win32 HWHEEL: positive = right), and
/// CDP `deltaX > 0` is also right, so the sign is kept.
#[cfg(windows)]
fn cdp_delta_x_for_ticks(ticks: i32) -> f64 {
    f64::from(ticks.saturating_mul(WHEEL_DELTA))
}

/// dy > 0 scrolls UP on every route (Win32 WHEEL: positive = up), but CDP
/// `deltaY > 0` is down, so the sign is flipped.
#[cfg(windows)]
fn cdp_delta_y_for_ticks(ticks: i32) -> f64 {
    -f64::from(ticks.saturating_mul(WHEEL_DELTA))
}

#[cfg(windows)]
fn verify_cdp_scroll_delta(
    verify_timeout_ms: u32,
    before: &synapse_a11y::CdpScrollState,
    after: &synapse_a11y::CdpScrollState,
) -> Result<ActPostcondition, ErrorData> {
    let before_signature = hash_json(before)?;
    let after_signature = hash_json(after)?;
    if before == after {
        return Err(no_observed_delta_error(
            "act_scroll",
            "cdp_node.scroll_state",
            verify_timeout_ms,
            before_signature,
            after_signature,
            json!({
                "before": before,
                "after": after,
            }),
        ));
    }
    Ok(postcondition_observed_delta(
        "act_scroll",
        "cdp_node.scroll_state",
        before_signature,
        after_signature,
        "observed target DOM scroll state change after CDP wheel dispatch",
    ))
}

fn scroll_actions(params: &ActScrollParams) -> Result<Vec<Action>, ErrorData> {
    if !params.smooth {
        return Ok(vec![scroll_action(
            params.dy,
            params.dx,
            params.at.map(Into::into),
        )]);
    }
    let step_count = smooth_step_count(params.dy, params.dx);
    let capacity = usize::try_from(step_count).map_err(|_err| {
        mcp_error(
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "act_scroll smooth=true step count cannot fit in memory",
        )
    })?;
    let mut actions = Vec::with_capacity(capacity);
    let mut vertical_ticks_remaining = params.dy;
    let mut horizontal_ticks_remaining = params.dx;
    for step_index in 0..step_count {
        let vertical_tick = take_tick(&mut vertical_ticks_remaining);
        let horizontal_tick = take_tick(&mut horizontal_ticks_remaining);
        actions.push(scroll_action(
            vertical_tick,
            horizontal_tick,
            if step_index == 0 {
                params.at.map(Into::into)
            } else {
                None
            },
        ));
    }
    Ok(actions)
}

const fn scroll_action(dy: i32, dx: i32, at: Option<Point>) -> Action {
    Action::MouseScroll {
        dy,
        dx,
        at,
        backend: Backend::Auto,
    }
}

fn smooth_step_count(dy: i32, dx: i32) -> u32 {
    dy.unsigned_abs().max(dx.unsigned_abs())
}

fn take_tick(value: &mut i32) -> i32 {
    match (*value).cmp(&0) {
        std::cmp::Ordering::Less => {
            *value += 1;
            -1
        }
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => {
            *value -= 1;
            1
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct ScrollDispatchResult {
    backend_used: &'static str,
    wheel_event_count: usize,
    pacing: ScrollPacing,
}

#[cfg(windows)]
async fn execute_native_scroll(
    handle: &ActionHandle,
    params: &ActScrollParams,
    checked_window_hwnd: Option<i64>,
) -> Result<ScrollDispatchResult, ErrorData> {
    let at = params.at.map(Into::into);
    let require_foreground = at.is_none();
    let (target, cursor) = if at.is_some() {
        let hwnd = checked_scroll_window(checked_window_hwnd)?;
        let target = handle
            .delivery_target_for_window(hwnd, "coordinate_scroll")
            .map_err(|error| action_error_to_mcp(&error))?;
        (target, None)
    } else {
        let target = handle
            .delivery_target()
            .map_err(|error| action_error_to_mcp(&error))?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::ACTION_INPUT_TARGET_REQUIRED,
                    format!("Wheel input refused before delivery: no intended window. {INPUT_TARGET_REMEDIATION}"),
                )
            })?;
        ensure_pointer_foreground(&target).map_err(|error| action_error_to_mcp(&error))?;
        let cursor = cursor_position().map_err(|error| action_error_to_mcp(&error))?;
        (target, Some(cursor))
    };
    let point =
        bound_wheel_point(&target, cursor, at).map_err(|error| action_error_to_mcp(&error))?;
    dispatch_targeted_scroll_actions(params, point, &target, require_foreground, |_| {}).await
}

#[cfg(all(windows, test))]
async fn execute_targeted_scroll_actions_platform(
    params: &ActScrollParams,
    point: Point,
    checked_window_hwnd: Option<i64>,
) -> Result<ScrollDispatchResult, ErrorData> {
    execute_targeted_scroll_actions_platform_with(params, point, checked_window_hwnd, |_| {}).await
}

#[cfg(all(windows, test))]
async fn execute_targeted_scroll_actions_platform_with(
    params: &ActScrollParams,
    point: Point,
    checked_window_hwnd: Option<i64>,
    before_post: impl FnMut(usize),
) -> Result<ScrollDispatchResult, ErrorData> {
    let target = InputDeliveryTarget::resolve_window(
        checked_scroll_window(checked_window_hwnd)?,
        "coordinate_scroll",
    )
    .map_err(|error| action_error_to_mcp(&error))?;
    dispatch_targeted_scroll_actions(params, point, &target, false, before_post).await
}

#[cfg(windows)]
fn checked_scroll_window(checked_window_hwnd: Option<i64>) -> Result<i64, ErrorData> {
    checked_window_hwnd.ok_or_else(|| {
        mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "coordinate scroll has no checked native target; no wheel input sent",
        )
    })
}

#[cfg(windows)]
async fn dispatch_targeted_scroll_actions(
    params: &ActScrollParams,
    point: Point,
    target: &InputDeliveryTarget,
    require_foreground: bool,
    mut before_post: impl FnMut(usize),
) -> Result<ScrollDispatchResult, ErrorData> {
    let readback = windows_hwnd_message_scroll_readback(point, target.root_hwnd)
        .map_err(|error| action_error_to_mcp(&error))?;
    // #2401: smooth steps run only on the foreground route; the posted route at
    // a point sends every message back to back, and the reply reads the same
    // `pacing`.
    let pacing = ScrollPacing::new(params.smooth, require_foreground, params.dy, params.dx);
    let mut dy = params.dy;
    let mut dx = params.dx;
    let mut wheel_event_count = 0_usize;
    for step in 0..pacing.steps() {
        let (vertical_notches, horizontal_notches) = if pacing.smooth() {
            (take_tick(&mut dy), take_tick(&mut dx))
        } else {
            (dy, dx)
        };
        let (vertical, horizontal) = WheelPlan::for_scroll(vertical_notches, horizontal_notches)
            .map_err(|error| scroll_dispatch_error(&error, wheel_event_count))?;
        for (message, plan, axis) in [
            (WM_MOUSEWHEEL, vertical, "vertical"),
            (WM_MOUSEHWHEEL, horizontal, "horizontal"),
        ] {
            for &delta in plan.messages() {
                before_post(wheel_event_count);
                if require_foreground {
                    ensure_pointer_foreground(target)
                        .map_err(|error| scroll_dispatch_error(&error, wheel_event_count))?;
                }
                checked_wheel_post(target, readback.hwnd, message, delta, point)
                    .map_err(|error| scroll_dispatch_error(&error, wheel_event_count))?;
                wheel_event_count += 1;
                tracing::info!(
                    code = "M2_ACT_SCROLL_HWND_MESSAGE",
                    kind = "act_scroll",
                    target_hwnd = readback.hwnd,
                    target_class = %readback.class_name,
                    screen_x = point.x,
                    screen_y = point.y,
                    delta = i32::from(delta),
                    axis,
                    "readback=window_message tool=act_scroll targeted_scroll_after"
                );
            }
        }
        if step + 1 < pacing.steps() {
            tokio::time::sleep(Duration::from_millis(u64::from(pacing.interval_ms()))).await;
        }
    }
    Ok(ScrollDispatchResult {
        backend_used: "software_window_message",
        wheel_event_count,
        pacing,
    })
}

#[cfg(windows)]
fn checked_wheel_post(
    target: &InputDeliveryTarget,
    recipient_hwnd: i64,
    message: u32,
    delta: i16,
    point: Point,
) -> Result<(), ActionError> {
    let point = bound_wheel_point(target, None, Some(point))?;
    // Re-hit-test after the previous post or smooth delay. Keep the original
    // recipient bound too, so reparenting cannot redirect the next message.
    let hit = root_hwnd_at_point(point).ok_or_else(|| ActionError::TargetInvalid {
        detail: format!("wheel point {point:?} is no longer over a live window"),
    })?;
    target.ensure_window_identity(hit)?;
    target.ensure_window_identity(recipient_hwnd)?;
    post_wheel_message(recipient_hwnd, message, delta, point, target.root_hwnd)
}

#[cfg(windows)]
fn bound_wheel_point(
    target: &InputDeliveryTarget,
    cursor: Option<Point>,
    explicit_point: Option<Point>,
) -> Result<Point, ActionError> {
    let rect = window_rect(hwnd_from_i64(target.root_hwnd)?)?;
    wheel_delivery_point(
        Rect {
            x: rect.left,
            y: rect.top,
            w: rect.right.saturating_sub(rect.left),
            h: rect.bottom.saturating_sub(rect.top),
        },
        cursor,
        explicit_point,
    )
}

#[cfg(not(windows))]
async fn execute_native_scroll(
    handle: &ActionHandle,
    params: &ActScrollParams,
    _checked_window_hwnd: Option<i64>,
) -> Result<ScrollDispatchResult, ErrorData> {
    if let Some(point) = params.at.map(Point::from) {
        return Err(action_error_to_mcp(&ActionError::BackendUnavailable {
            detail: format!(
                "act_scroll at={point:?} targeted window-message path requires Windows"
            ),
        }));
    }
    let actions = scroll_actions(params)?;
    let wheel_event_count = actions.len();
    execute_scroll_actions(handle, actions, params.smooth).await?;
    Ok(ScrollDispatchResult {
        backend_used: "software",
        wheel_event_count,
        pacing: ScrollPacing::new(params.smooth, true, params.dy, params.dx),
    })
}

/// A wheel refusal names how many events reached the target before it
/// stopped (`delivered_wheel_event_count`); any earlier delivery is marked.
#[cfg(windows)]
fn scroll_dispatch_error(error: &ActionError, delivered_count: usize) -> ErrorData {
    let mut error = action_error_to_mcp(error);
    if let Value::Object(data) = error.data.get_or_insert_with(|| json!({})) {
        data.insert(
            "delivered_wheel_event_count".to_owned(),
            json!(delivered_count),
        );
    }
    if delivered_count == 0 {
        error
    } else {
        super::postcondition::mark_input_delivered(error)
    }
}

#[cfg(not(windows))]
async fn execute_scroll_actions(
    handle: &ActionHandle,
    actions: Vec<Action>,
    smooth: bool,
) -> Result<(), ErrorData> {
    let last_index = actions.len().saturating_sub(1);
    for (index, action) in actions.into_iter().enumerate() {
        handle
            .execute(action)
            .await
            .map_err(|error| action_error_to_mcp(&error))?;
        if smooth && index < last_index {
            tokio::time::sleep(Duration::from_millis(u64::from(SMOOTH_SCROLL_INTERVAL_MS))).await;
        }
    }
    Ok(())
}

/// The reply of one scroll. `smooth`, `smooth_interval_ms` and
/// `scheduled_smooth_total_ms` come from `pacing`, what the route really ran
/// (#2401, #2395), never from the request's `smooth`.
fn response(
    params: &ActScrollParams,
    scrolled: bool,
    wheel_event_count: usize,
    backend_used: &'static str,
    pacing: ScrollPacing,
    started: Instant,
) -> ActScrollResponse {
    let wheel_event_count = u32::try_from(wheel_event_count).unwrap_or(u32::MAX);
    let backend_tier_used = scroll_backend_tier_used(params, scrolled, backend_used);
    ActScrollResponse {
        ok: true,
        dy: params.dy,
        dx: params.dx,
        smooth: pacing.smooth(),
        scrolled,
        wheel_event_count,
        smooth_interval_ms: pacing.interval_ms(),
        scheduled_smooth_total_ms: pacing.scheduled_total_ms(),
        backend_used: backend_used.to_owned(),
        backend_tier_used: backend_tier_used.to_owned(),
        required_foreground: scroll_required_foreground(backend_tier_used),
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
        postcondition: postcondition_not_requested(
            "act_scroll",
            response_source_of_truth(params, scrolled),
        ),
    }
}

fn response_source_of_truth(params: &ActScrollParams, scrolled: bool) -> &'static str {
    if !scrolled {
        return "not_applicable.no_scroll";
    }
    if params.target.is_some() {
        "cdp_node.scroll_state"
    } else {
        "target_point_pixels_or_foreground_ui"
    }
}

fn scroll_backend_tier_used(
    params: &ActScrollParams,
    scrolled: bool,
    backend_used: &'static str,
) -> &'static str {
    if !scrolled {
        return "none";
    }
    if backend_used == "uia_scroll_pattern" || backend_used == "uia_scroll_item_pattern" {
        return "uia";
    }
    if params.target.is_some() || backend_used == "cdp" {
        return "cdp";
    }
    if params.at.is_some() {
        return "postmessage";
    }
    "foreground"
}

fn scroll_required_foreground(backend_tier_used: &str) -> bool {
    backend_tier_used == "foreground"
}

async fn execute_recording(
    recording: &RecordingBackend,
    actions: &[Action],
    params: &ActScrollParams,
) -> Result<(), ErrorData> {
    let mut new_events = Vec::new();
    let mut emit_state = EmitState::new();
    let last_index = actions.len().saturating_sub(1);
    for (index, action) in actions.iter().enumerate() {
        new_events.extend(
            recording
                .execute_capturing(action, &mut emit_state)
                .map_err(|error| action_error_to_mcp(&error))?,
        );
        if params.smooth && index < last_index {
            tokio::time::sleep(Duration::from_millis(u64::from(SMOOTH_SCROLL_INTERVAL_MS))).await;
        }
    }
    log_recording_readback(recording.event_count(), &new_events, params);
    Ok(())
}

fn execute_recording_noop(recording: &RecordingBackend, params: &ActScrollParams) {
    log_recording_readback(recording.event_count(), &[], params);
}

fn log_recording_readback(
    recorded_event_total: usize,
    new_events: &[RecordedInput],
    params: &ActScrollParams,
) {
    let event_sequence = event_sequence(new_events);
    let smooth_step_count = if params.smooth {
        smooth_step_count(params.dy, params.dx)
    } else {
        0
    };
    tracing::info!(
        code = "M2_ACT_SCROLL_RECORDING_READBACK",
        kind = "act_scroll",
        recorded_event_total,
        new_event_count = new_events.len(),
        dy = params.dy,
        dx = params.dx,
        smooth = params.smooth,
        smooth_step_count,
        smooth_interval_ms = if params.smooth {
            SMOOTH_SCROLL_INTERVAL_MS
        } else {
            0
        },
        scheduled_smooth_total_ms =
            ScrollPacing::new(params.smooth, true, params.dy, params.dx).scheduled_total_ms(),
        event_sequence,
        "readback=recording_backend tool=act_scroll after_events_readback"
    );
}

fn event_sequence(events: &[RecordedInput]) -> String {
    events.iter().map(event_label).collect::<Vec<_>>().join(">")
}

fn event_label(event: &RecordedInput) -> String {
    match event {
        RecordedInput::MouseScroll { dy, dx, at } => {
            format!("mouse_scroll:dy={dy}:dx={dx}:at={}", at_label(*at))
        }
        other => other.log_label(),
    }
}

fn at_label(at: Option<Point>) -> String {
    at.map_or_else(
        || "none".to_owned(),
        |point| format!("screen({},{})", point.x, point.y),
    )
}

fn action_error_to_mcp(error: &ActionError) -> ErrorData {
    crate::m2::action_error_to_mcp(error)
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct HwndMessageScrollReadback {
    hwnd: i64,
    class_name: String,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct WindowCandidate {
    hwnd: HWND,
    class_name: String,
}

/// Top-level window under a screen point, for claim scoping of a coordinate
/// scroll (the wheel message is delivered to whatever window is there).
#[cfg(windows)]
pub(crate) fn root_hwnd_at_point(point: Point) -> Option<i64> {
    // SAFETY: WindowFromPoint takes a POINT by value and returns a window handle or null; the
    // handle is never dereferenced and null is handled below.
    let seed = unsafe {
        WindowFromPoint(WinPoint {
            x: point.x,
            y: point.y,
        })
    };
    if seed.0.is_null() {
        return None;
    }
    // SAFETY: `seed` is a non-null handle just returned by WindowFromPoint; GetAncestor takes it
    // by value and returns null on failure, which falls back to `seed`.
    let root = unsafe { GetAncestor(seed, GA_ROOT) };
    let root = if root.0.is_null() { seed } else { root };
    Some(hwnd_to_i64(root))
}

#[cfg(not(windows))]
pub(crate) fn root_hwnd_at_point(_point: Point) -> Option<i64> {
    None
}

#[cfg(windows)]
fn windows_hwnd_message_scroll_readback(
    point: Point,
    checked_window_hwnd: i64,
) -> Result<HwndMessageScrollReadback, ActionError> {
    // SAFETY: WindowFromPoint takes a POINT by value and returns a window handle or null; the
    // handle is never dereferenced and null is handled below.
    let seed = unsafe {
        WindowFromPoint(WinPoint {
            x: point.x,
            y: point.y,
        })
    };
    if seed.0.is_null() {
        return Err(ActionError::TargetInvalid {
            detail: format!("act_scroll at point {point:?} is not over a live window"),
        });
    }
    let root_hwnd = synapse_action::delivery_target::live_root_hwnd(
        hwnd_to_i64(seed),
        &format!("act_scroll window under point {point:?}"),
    )?;
    if root_hwnd != checked_window_hwnd {
        return Err(ActionError::TargetInvalid {
            detail: format!(
                "coordinate scroll point is over hwnd 0x{root_hwnd:x}, not checked window 0x{checked_window_hwnd:x}; no wheel input sent"
            ),
        });
    }
    let root = hwnd_from_i64(root_hwnd)?;

    let target = hit_test_hwnd_for_screen_point(seed, root, point)?;
    let _ = screen_lparam(point)?;
    Ok(HwndMessageScrollReadback {
        hwnd: hwnd_to_i64(target.hwnd),
        class_name: target.class_name,
    })
}

#[cfg(windows)]
fn hit_test_hwnd_for_screen_point(
    seed: HWND,
    root: HWND,
    point: Point,
) -> Result<WindowCandidate, ActionError> {
    let root_rect = window_rect(root)?;
    if !rect_contains_point(&root_rect, point) {
        return Err(ActionError::TargetInvalid {
            detail: format!(
                "act_scroll point {point:?} is outside root hwnd 0x{:x} rect {:?}",
                hwnd_to_i64(root),
                rect_tuple(&root_rect)
            ),
        });
    }

    // SAFETY: HWND is a plain handle value that is never dereferenced; IsWindowVisible only
    // reads it and returns FALSE for a stale or invalid handle.
    if let Ok(seed_rect) = window_rect(seed)
        && unsafe { IsWindowVisible(seed) }.as_bool()
        && rect_contains_point(&seed_rect, point)
        && rect_area(&seed_rect) > 0
    {
        return Ok(WindowCandidate {
            hwnd: seed,
            class_name: window_class_name(seed),
        });
    }

    // #197: z-order aware descent (topmost hittable sibling at each level), not smallest-area.
    let hwnd = crate::m2::click::hit_test::deepest_hit_hwnd(root, point);
    Ok(WindowCandidate {
        hwnd,
        class_name: window_class_name(hwnd),
    })
}

#[cfg(windows)]
fn post_wheel_message(
    hwnd: i64,
    message: u32,
    delta: i16,
    screen_point: Point,
    checked_window_hwnd: i64,
) -> Result<(), ActionError> {
    let root =
        synapse_action::delivery_target::live_root_hwnd(hwnd, "act_scroll delivery target check")?;
    if root != checked_window_hwnd {
        return Err(ActionError::TargetInvalid {
            detail: format!(
                "act_scroll delivery target no longer belongs to checked window 0x{checked_window_hwnd:x}; no wheel message posted"
            ),
        });
    }
    let hwnd = hwnd_from_i64(hwnd)?;
    let wparam = wheel_wparam(delta)?;
    let lparam = screen_lparam(screen_point)?;
    // SAFETY: wparam/lparam are plain integers (wheel delta and packed screen coordinates),
    // never pointers, so nothing can dangle in the target's queue; a stale HWND makes
    // PostMessageW return an error, which is mapped below.
    unsafe { PostMessageW(Some(hwnd), message, wparam, lparam) }.map_err(|error| {
        ActionError::BackendUnavailable {
            detail: format!(
                "PostMessageW act_scroll wheel message 0x{message:x} failed for hwnd 0x{:x} screen_point={screen_point:?} delta={delta}: {error}",
                hwnd_to_i64(hwnd)
            ),
        }
    })
}

/// #2351: the window messages one posted scroll of `notches` sends. One notch
/// is one message of `WHEEL_DELTA` (120) with the notch's sign, like a real
/// wheel: apps such as Firefox scroll far less for one oversized message.
/// More notches than `MAX_TARGETED_WHEEL_MESSAGES` are refused before
/// anything is posted. Pure value object; the posting stays in
/// `dispatch_targeted_scroll_actions`.
#[cfg(windows)]
struct WheelPlan {
    messages: Vec<i16>,
}

#[cfg(windows)]
impl WheelPlan {
    /// The plan for `notches` (positive = up / right, negative = down /
    /// left); refused when it needs more than `MAX_TARGETED_WHEEL_MESSAGES`
    /// messages.
    fn for_notches(notches: i32) -> Result<Self, ActionError> {
        let count = usize::try_from(notches.unsigned_abs()).unwrap_or(usize::MAX);
        if count > MAX_TARGETED_WHEEL_MESSAGES {
            return Err(ActionError::TargetInvalid {
                detail: format!(
                    "act_scroll targeted wheel message count exceeds {MAX_TARGETED_WHEEL_MESSAGES} for ticks={notches}"
                ),
            });
        }
        let delta = WHEEL_DELTA.saturating_mul(notches.signum());
        let delta = i16::try_from(delta).map_err(|error| ActionError::TargetInvalid {
            detail: format!(
                "act_scroll wheel delta {delta} cannot fit WM_MOUSE*WHEEL i16: {error}"
            ),
        })?;
        Ok(Self {
            messages: vec![delta; count],
        })
    }

    /// The wheel deltas to post, in order: one per message.
    const fn messages(&self) -> &[i16] {
        self.messages.as_slice()
    }

    /// #2401: the vertical and the horizontal plan of one scroll step, made
    /// together: refused when the two need more than
    /// `MAX_TARGETED_WHEEL_MESSAGES` messages TOGETHER, so nothing is posted.
    /// The posting loop plans only through this.
    fn for_scroll(dy: i32, dx: i32) -> Result<(Self, Self), ActionError> {
        let total = usize::try_from(dy.unsigned_abs())
            .unwrap_or(usize::MAX)
            .saturating_add(usize::try_from(dx.unsigned_abs()).unwrap_or(usize::MAX));
        if total > MAX_TARGETED_WHEEL_MESSAGES {
            return Err(ActionError::TargetInvalid {
                detail: format!(
                    "act_scroll targeted wheel message count {total} exceeds {MAX_TARGETED_WHEEL_MESSAGES} for dy={dy} dx={dx}"
                ),
            });
        }
        Ok((Self::for_notches(dy)?, Self::for_notches(dx)?))
    }
}

#[cfg(all(test, windows))]
#[path = "wheel_plan_table_2351.rs"]
mod wheel_plan_table_2351;

/// #2401 (#2395): how one scroll was paced, taken from what really runs: smooth
/// steps with `SMOOTH_SCROLL_INTERVAL_MS` between two of them, or one step
/// that carries every message back to back. The reply's `smooth`,
/// `smooth_interval_ms` and `scheduled_smooth_total_ms` come only from here,
/// never from the request. Pure value object.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct ScrollPacing {
    smooth: bool,
    steps: u32,
}

impl ScrollPacing {
    /// `smooth_asked` is the request's `smooth`; `route_pauses` says whether
    /// the route that runs the scroll waits between steps (the foreground
    /// window-message route, CDP, the recording backend, the non-Windows
    /// backend); the posted route at a point and the UIA pattern never do.
    /// A smooth scroll runs one step per notch of the longer axis; any other
    /// scroll runs one step.
    fn new(smooth_asked: bool, route_pauses: bool, dy: i32, dx: i32) -> Self {
        let smooth = smooth_asked && route_pauses;
        Self {
            smooth,
            steps: if smooth { smooth_step_count(dy, dx) } else { 1 },
        }
    }

    /// How many steps run (0 when a smooth scroll has nothing to scroll).
    const fn steps(&self) -> u32 {
        self.steps
    }

    /// Whether smooth steps ran: asked, on a route that pauses, at least one step.
    const fn smooth(&self) -> bool {
        self.smooth && self.steps > 0
    }

    /// The pause between two smooth steps; 0 when the scroll was not smooth.
    const fn interval_ms(&self) -> u32 {
        if self.smooth() {
            SMOOTH_SCROLL_INTERVAL_MS
        } else {
            0
        }
    }

    /// The pauses the smooth steps took together: one fewer than the steps.
    const fn scheduled_total_ms(&self) -> u32 {
        if self.smooth() {
            self.steps
                .saturating_sub(1)
                .saturating_mul(SMOOTH_SCROLL_INTERVAL_MS)
        } else {
            0
        }
    }
}

#[cfg(all(test, windows))]
#[path = "wheel_plan_table_2401.rs"]
mod wheel_plan_table_2401;

#[cfg(windows)]
fn wheel_wparam(delta: i16) -> Result<WPARAM, ActionError> {
    let high_word = u32::from(u16::from_ne_bytes(delta.to_ne_bytes())) << 16;
    Ok(WPARAM(usize::try_from(high_word).map_err(|error| {
        ActionError::TargetInvalid {
            detail: format!("act_scroll wheel wParam overflowed usize: {error}"),
        }
    })?))
}

#[cfg(windows)]
fn screen_lparam(point: Point) -> Result<LPARAM, ActionError> {
    let x = i16::try_from(point.x).map_err(|error| ActionError::TargetInvalid {
        detail: format!(
            "act_scroll screen x {} cannot fit a WM_MOUSE*WHEEL lParam i16: {error}",
            point.x
        ),
    })?;
    let y = i16::try_from(point.y).map_err(|error| ActionError::TargetInvalid {
        detail: format!(
            "act_scroll screen y {} cannot fit a WM_MOUSE*WHEEL lParam i16: {error}",
            point.y
        ),
    })?;
    let packed = (u32::from(u16::from_ne_bytes(y.to_ne_bytes())) << 16)
        | u32::from(u16::from_ne_bytes(x.to_ne_bytes()));
    Ok(LPARAM(isize::try_from(packed).unwrap_or(isize::MAX)))
}

#[cfg(windows)]
fn window_rect(hwnd: HWND) -> Result<RECT, ActionError> {
    let mut rect = RECT::default();
    // SAFETY: the out-pointer refers to the live local `rect`; a stale HWND makes the call
    // return an error, which is mapped below.
    unsafe { GetWindowRect(hwnd, &raw mut rect) }.map_err(|error| {
        ActionError::ElementNotResolved {
            detail: format!(
                "GetWindowRect failed for act_scroll hwnd 0x{:x}: {error}",
                hwnd_to_i64(hwnd)
            ),
        }
    })?;
    Ok(rect)
}

#[cfg(windows)]
fn rect_contains_point(rect: &RECT, point: Point) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

#[cfg(windows)]
fn rect_area(rect: &RECT) -> i64 {
    let width = i64::from(rect.right.saturating_sub(rect.left).max(0));
    let height = i64::from(rect.bottom.saturating_sub(rect.top).max(0));
    width.saturating_mul(height)
}

#[cfg(windows)]
fn rect_tuple(rect: &RECT) -> (i32, i32, i32, i32) {
    (rect.left, rect.top, rect.right, rect.bottom)
}

#[cfg(windows)]
fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = vec![0_u16; 256];
    // SAFETY: `buffer` is a live, writable UTF-16 slice and GetClassNameW writes at most its
    // length; a stale HWND returns 0, which yields an empty class name.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
}

#[cfg(windows)]
fn hwnd_from_i64(hwnd: i64) -> Result<HWND, ActionError> {
    if hwnd == 0 {
        return Err(ActionError::TargetInvalid {
            detail: "act_scroll target hwnd is null".to_owned(),
        });
    }
    Ok(HWND(hwnd as isize as *mut c_void))
}

#[cfg(windows)]
fn hwnd_to_i64(hwnd: HWND) -> i64 {
    hwnd.0 as isize as i64
}

#[cfg(all(test, windows))]
#[path = "tests/scroll_direction_192.rs"]
mod scroll_direction_192;

#[cfg(all(test, windows))]
#[path = "tests/wheel_reparent_62.rs"]
mod wheel_reparent_62;
