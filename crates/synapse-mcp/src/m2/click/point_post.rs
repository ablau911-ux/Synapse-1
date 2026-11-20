//! #107: target-scoped point clicks on a claimed native HWND must not require
//! OS foreground. Routing decision + background delivery for point targets.
//!
//! Delivery never uses SendInput: the point is resolved inside the claimed
//! root's child-window tree. Native buttons receive BM_CLICK; a WPF or XAML
//! control receives one UI Automation pattern click (#107 part d,
//! `point_element`); other controls receive WM_* mouse messages. The caller
//! verifies the target-window pixel delta; a missing delta fails closed.
//! Background delivery requires a live session `target_claim` on the root
//! (`ClaimedPointTarget`); a caller-supplied HWND alone never qualifies.

use std::time::Instant;

use rmcp::ErrorData;
use synapse_action::{DoubleClickTiming, cached_double_click_timing};
use synapse_core::{Backend, Point, error_codes};

use super::schema::{ActClickParams, ActClickResponse, ActClickTarget, ActClickTierAttempt};
use super::{
    CLICK_REASON_TARGET_INVALID, CLICK_TIER_POSTMESSAGE, CLICK_TIER_UIA, action_error_to_mcp,
    attach_click_tier_attempts, click_backend_tier_used, click_required_foreground,
    click_target_root_hwnd, click_tier_delivered, click_tier_failed,
    reject_click_modifiers_for_non_cdp, validate_click_params,
};
use crate::m1::mcp_error;
use crate::server::native_click_evidence::PatternClickEvidence;

#[cfg(windows)]
use super::click_reason_for_error_code;
#[cfg(windows)]
use super::element::{HwndMessageClickReadback, mouse_button_messages, post_mouse_message};
#[cfg(windows)]
use super::hwnd_post::{
    PostedMouseStep, class_accepts_double_clicks, double_click_message, observe_child_identity,
    posted_click_steps, revalidate_child_identity, snapshot_child_identity,
};
#[cfg(windows)]
use super::point_element::{PointClickPlan, PointElement, PointElementPicker};
#[cfg(windows)]
use crate::server::element_root::ElementRoot;
#[cfg(windows)]
use tokio::time::{Duration, sleep};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, BS_GROUPBOX, BS_TYPEMASK, GWL_STYLE, GetWindowLongW, WM_MOUSEMOVE,
};

/// Proof that the calling MCP session owns a live `target_claim` on
/// `root_hwnd`. Construct only from the session claim registry readback
/// (`SynapseService::target_claim_for_session`); a caller-supplied
/// `verify_target_window_hwnd` alone is never a claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClaimedPointTarget {
    root_hwnd: i64,
    claim_generation: u64,
}

impl ClaimedPointTarget {
    pub(crate) const fn from_session_claim(root_hwnd: i64, claim_generation: u64) -> Self {
        Self {
            root_hwnd,
            claim_generation,
        }
    }
}

/// Delivery route chosen for a point click from its claim and OS foreground state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PointClickRoute {
    /// No target window, or target already foreground: existing foreground-tier behavior.
    Direct,
    /// Session-claimed target is not foreground: deliver through its child-window tree.
    PostMessage(ClaimedPointTarget),
    /// Claimed target is not foreground and the backend cannot run in background: fail closed.
    RefuseForeground,
    /// Background target window is not claimed by this session: fail closed.
    RefuseUnclaimed,
}

/// Pure routing decision. `foreground_root` is the OS foreground top-level HWND,
/// or `None` when the OS has no foreground window (locked screen, desktop focus);
/// `claim` is this session's live claim readback for `target_root`. A missing
/// foreground never equals the target, so a claimed target still routes to
/// background delivery (#7) while an unclaimed one is refused.
pub(crate) fn point_click_route(
    params: &ActClickParams,
    target_root: Option<i64>,
    foreground_root: Option<i64>,
    claim: Option<ClaimedPointTarget>,
) -> PointClickRoute {
    if !matches!(params.target, ActClickTarget::Point(_)) {
        return PointClickRoute::Direct;
    }
    let Some(target) = target_root else {
        return PointClickRoute::Direct;
    };
    if foreground_root == Some(target) {
        return PointClickRoute::Direct;
    }
    let Some(claim) = claim.filter(|claim| claim.root_hwnd == target) else {
        return PointClickRoute::RefuseUnclaimed;
    };
    match params.backend {
        Backend::Auto | Backend::Software => PointClickRoute::PostMessage(claim),
        Backend::Vigem | Backend::Hardware => PointClickRoute::RefuseForeground,
    }
}

/// True when the request is a session-claimed point click that background
/// PostMessage delivery can serve with no OS foreground at all (#7). The action
/// gate uses it to skip the live-foreground requirement; unclaimed, hardware and
/// targetless requests stay fail-closed.
pub(crate) fn claimed_background_point_possible(
    params: &ActClickParams,
    target_root: Option<i64>,
    claim: Option<ClaimedPointTarget>,
) -> bool {
    matches!(
        point_click_route(params, target_root, None, claim),
        PointClickRoute::PostMessage(_)
    )
}

/// #107: whether a click reads its target root before routing. `verify_delta`
/// always needs it (capture region); a point click that names its window
/// (`verify_target_window_hwnd`) needs it too, so a claimed background window
/// is posted to with `verify_delta=false` instead of reaching the foreground
/// tier. A bare point stays on the direct route.
pub(crate) const fn point_click_resolves_root(params: &ActClickParams) -> bool {
    params.verify_delta
        || (matches!(params.target, ActClickTarget::Point(_))
            && params.verify_target_window_hwnd.is_some())
}

/// Reads the live OS foreground and returns the route for this request.
/// No foreground window at all (`A11Y_NO_FOREGROUND`) is a valid state routed
/// as "target is not foreground" (#7); any other readback failure is an error
/// (fail closed), not a Direct route.
pub(crate) fn current_point_click_route(
    params: &ActClickParams,
    target_root: Option<i64>,
    claim: Option<ClaimedPointTarget>,
) -> Result<PointClickRoute, ErrorData> {
    if target_root.is_none() || !matches!(params.target, ActClickTarget::Point(_)) {
        return Ok(PointClickRoute::Direct);
    }
    let foreground_root = match synapse_a11y::current_foreground_context() {
        Ok(foreground) => Ok(synapse_a11y::top_level_root_hwnd(foreground.hwnd).map_err(
            |error| {
                let error = synapse_a11y::as_foreground_error(error);
                mcp_error(
                    error.code(),
                    format!(
                        "act_click foreground HWND 0x{:x} root readback failed: {error}",
                        foreground.hwnd
                    ),
                )
            },
        )?),
        Err(error) => Err(error),
    };
    current_point_click_route_with(params, target_root, claim, || foreground_root)
}

pub(crate) fn current_point_click_route_with(
    params: &ActClickParams,
    target_root: Option<i64>,
    claim: Option<ClaimedPointTarget>,
    foreground_root: impl FnOnce() -> Result<i64, synapse_a11y::A11yError>,
) -> Result<PointClickRoute, ErrorData> {
    if target_root.is_none() || !matches!(params.target, ActClickTarget::Point(_)) {
        return Ok(PointClickRoute::Direct);
    }
    let foreground_root = match foreground_root() {
        Ok(root) => Some(root),
        Err(error) if error.code() == error_codes::A11Y_NO_FOREGROUND => None,
        Err(error) => {
            return Err(mcp_error(
                error.code(),
                format!("act_click foreground readback failed: {error}"),
            ));
        }
    };
    Ok(point_click_route(
        params,
        target_root,
        foreground_root,
        claim,
    ))
}

/// Error for a background point target this session has not claimed.
pub(crate) fn unclaimed_point_target_error(target_root: Option<i64>) -> ErrorData {
    let target = target_root.map_or_else(|| "none".to_owned(), |hwnd| format!("0x{hwnd:x}"));
    mcp_error(
        error_codes::TARGET_CLAIM_NOT_FOUND,
        format!(
            "act_click refused before delivery: background point target window {target} is not claimed by this MCP session; verify_target_window_hwnd alone never authorizes background PostMessage delivery. Call target_claim for the window first; no input was sent"
        ),
    )
}

/// Point branch of the generic PostMessage tier: it carries no session-claim
/// proof, so it always refuses. Claimed point clicks use
/// [`execute_claimed_point_postmessage_click`].
pub(super) fn refuse_unproven_point_postmessage(
    params: &ActClickParams,
    mut prior_attempts: Vec<ActClickTierAttempt>,
) -> ErrorData {
    let error = unclaimed_point_target_error(params.verify_target_window_hwnd);
    prior_attempts.push(click_tier_failed(
        CLICK_TIER_POSTMESSAGE,
        CLICK_REASON_TARGET_INVALID,
        error_codes::TARGET_CLAIM_NOT_FOUND,
        false,
        error.message.to_string(),
    ));
    attach_click_tier_attempts(error, prior_attempts)
}

/// Delivers a point click to a session-claimed root's child-window tree via PostMessage.
pub(crate) async fn execute_claimed_point_postmessage_click(
    params: &ActClickParams,
    claim: ClaimedPointTarget,
    tier_attempts: Vec<ActClickTierAttempt>,
) -> Result<ActClickResponse, ErrorData> {
    validate_click_params(params)?;
    reject_click_modifiers_for_non_cdp(params, "HWND PostMessage point delivery")?;
    let ActClickTarget::Point(point) = &params.target else {
        return Err(refuse_unproven_point_postmessage(params, tier_attempts));
    };
    if click_target_root_hwnd(params)? != Some(claim.root_hwnd) {
        return Err(refuse_unproven_point_postmessage(params, tier_attempts));
    }
    tracing::info!(
        code = "M2_ACT_CLICK_CLAIMED_POINT_POSTMESSAGE",
        kind = "act_click",
        root_hwnd = claim.root_hwnd,
        claim_generation = claim.claim_generation,
        "act_click point PostMessage authorized by session target_claim"
    );
    execute_point_postmessage_click(
        params,
        Point {
            x: point.x,
            y: point.y,
        },
        claim.root_hwnd,
        tier_attempts,
        cached_double_click_timing(),
        Instant::now(),
    )
    .await
}

/// Delivers a point click to the claimed root: one UI Automation pattern click
/// on a WPF/XAML control (#107 part d), else HWND PostMessage to its
/// child-window tree. The reply says which one ran.
async fn execute_point_postmessage_click(
    params: &ActClickParams,
    point: Point,
    root_hwnd: i64,
    mut tier_attempts: Vec<ActClickTierAttempt>,
    timing: DoubleClickTiming,
    started: Instant,
) -> Result<ActClickResponse, ErrorData> {
    let pattern = post_point_clicks(params, point, root_hwnd, timing, &tier_attempts).await?;
    tier_attempts.push(match &pattern {
        None => click_tier_delivered(
            CLICK_TIER_POSTMESSAGE,
            false,
            "point click delivered to the claimed target child-window tree through HWND PostMessage (BM_CLICK for native buttons); OS foreground unchanged",
        ),
        Some(evidence) => click_tier_delivered(
            CLICK_TIER_UIA,
            false,
            format!(
                "point click delivered as one UI Automation pattern click on the WPF/XAML control at the point (evidence {}); no window message posted",
                evidence.source_of_truth()
            ),
        ),
    });
    Ok(ActClickResponse {
        ok: true,
        used_invoke_pattern: pattern.is_some(),
        backend_used: if pattern.is_some() {
            "uia"
        } else {
            "software_window_message"
        }
        .to_owned(),
        backend_tier_used: click_backend_tier_used(&tier_attempts),
        required_foreground: click_required_foreground(&tier_attempts),
        tier_attempts,
        // #107 part d: the pattern's own state read, when it proves the click.
        postcondition: pattern
            .as_ref()
            .and_then(PatternClickEvidence::postcondition)
            .unwrap_or_else(super::schema::postcondition_not_requested),
        press_hold_ms: params.hold_ms,
        double_click_window_ms: timing.window_ms,
        inter_click_delay_ms: timing.inter_click_delay_ms,
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}

/// #107 part d: the one hook of the point route. Reads the child window under
/// the point, asks `PointClickPlan` for the ONE delivery and runs it; each
/// delivery's failure carries its own tier attempt. Returns the pattern
/// evidence when the delivery was a UI Automation pattern click.
#[cfg(windows)]
async fn post_point_clicks(
    params: &ActClickParams,
    point: Point,
    root_hwnd: i64,
    timing: DoubleClickTiming,
    prior_attempts: &[ActClickTierAttempt],
) -> Result<Option<PatternClickEvidence>, ErrorData> {
    let posted_failure =
        |error: ErrorData| super::element::postmessage_failure(error, prior_attempts.to_vec());
    let readback = super::element::hwnd_message_readback_for_root(root_hwnd, point, params.button)
        .map_err(|error| {
            posted_failure(super::element::mark_click_not_dispatched(
                action_error_to_mcp(&error),
            ))
        })?;
    match PointClickPlan::choose(
        params.button,
        params.clicks,
        is_native_button(&readback),
        || element_at_point(root_hwnd, point),
    ) {
        PointClickPlan::BmClick => post_native_button_click(params, &readback)
            .map(|()| None)
            .map_err(posted_failure),
        PointClickPlan::UiaPattern(element) => {
            click_point_element(&element, root_hwnd, prior_attempts).map(Some)
        }
        PointClickPlan::PostedMessages => post_hwnd_clicks(params, &readback, timing)
            .await
            .map(|()| None)
            .map_err(posted_failure),
    }
}

/// #107 part d: the control at the point inside the claimed window, read from
/// its accessibility tree (the walk `find` uses), with its UIA framework.
/// `None` (posted messages, as before) when the tree or the framework cannot
/// be read.
#[cfg(windows)]
fn element_at_point(root_hwnd: i64, point: Point) -> Option<PointElement> {
    let unread = |step: &'static str, error: synapse_a11y::A11yError| {
        tracing::warn!(
            code = "M2_ACT_CLICK_POINT_ELEMENT_UNREAD",
            kind = "act_click",
            root_hwnd,
            step,
            error = %error,
            "act_click could not read the control at the point; posted messages are used"
        );
    };
    let tree = synapse_a11y::snapshot_window_from_hwnd(
        root_hwnd,
        crate::m1::MAX_PUBLIC_A11Y_SNAPSHOT_DEPTH,
    )
    .map_err(|error| unread("snapshot", error))
    .ok()?;
    let picker = PointElementPicker::new(tree.nodes);
    let node = picker.pick(point)?;
    let framework = synapse_a11y::element_framework_id(&node.element_id)
        .map_err(|error| unread("framework_id", error))
        .ok()?;
    Some(PointElement::new(
        node.element_id.clone(),
        framework,
        node.patterns.clone(),
    ))
}

/// #107 part d: the ONE UI Automation pattern click on the control at the
/// point, through the chain the element-id click uses
/// (`click_element_action`). The element must still belong to the claimed
/// root (the `target_act` check, `ElementRoot`); nothing is delivered
/// otherwise. No second delivery follows a failure.
#[cfg(windows)]
fn click_point_element(
    element: &PointElement,
    root_hwnd: i64,
    prior_attempts: &[ActClickTierAttempt],
) -> Result<PatternClickEvidence, ErrorData> {
    let element_id = element.element_id();
    let stop = |marked: ErrorData, code: &str| {
        let mut attempts = prior_attempts.to_vec();
        attempts.push(click_tier_failed(
            CLICK_TIER_UIA,
            click_reason_for_error_code(code),
            code,
            false,
            marked.message.to_string(),
        ));
        super::element::stop_click_router(
            attach_click_tier_attempts(marked, attempts),
            "point_uia_pattern_click_failed",
        )
    };
    if let Err(refusal) = ElementRoot::read(element_id, "act_click point click")
        .and_then(|element_root| element_root.ensure_in(root_hwnd))
    {
        let code = refusal
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(error_codes::ACTION_TARGET_INVALID)
            .to_owned();
        return Err(stop(
            super::element::mark_click_not_dispatched(refusal),
            &code,
        ));
    }
    tracing::info!(
        code = "M2_ACT_CLICK_POINT_UIA_PATTERN",
        kind = "act_click",
        element_id = %element_id,
        "act_click point click goes to the UI Automation pattern chain of the control at the point"
    );
    synapse_a11y::click_element_action(element_id)
        .map(PatternClickEvidence::of)
        .map_err(|error| {
            let code = error.code();
            let detail = mcp_error(
                code,
                format!(
                    "act_click point click via the UI Automation pattern chain on element {element_id} failed: {error}"
                ),
            );
            let marked = match error {
                synapse_a11y::A11yError::ClickDispatch {
                    delivered: true, ..
                } => crate::m2::postcondition::mark_input_delivered(detail),
                synapse_a11y::A11yError::ClickDispatch { .. } => {
                    crate::m2::postcondition::mark_input_possibly_delivered(detail)
                }
                _ => super::element::mark_click_not_dispatched(detail),
            };
            stop(marked, code)
        })
}

#[cfg(windows)]
fn is_native_button(readback: &HwndMessageClickReadback) -> bool {
    let class = readback.class_name.to_ascii_lowercase();
    if class != "button" && !class.starts_with("windowsforms10.button.") {
        return false;
    }
    let hwnd = windows::Win32::Foundation::HWND(readback.hwnd as isize as *mut std::ffi::c_void);
    // SAFETY: reads only the window's style; stale handles return zero and are
    // independently rejected by snapshot_child_identity before delivery.
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
    style & BS_TYPEMASK != BS_GROUPBOX
}

/// BM_CLICK activates WinForms buttons without their WM_LBUTTONUP hit test
/// against the foreground cover (#7). Send exactly once, with zero parameters;
/// the caller still requires a separate target-window effect readback.
#[cfg(windows)]
fn post_native_button_click(
    params: &ActClickParams,
    readback: &HwndMessageClickReadback,
) -> Result<(), ErrorData> {
    let to_mcp = |error: synapse_action::ActionError| posted_click_error(&error, false);
    let identity = snapshot_child_identity(readback.root_hwnd, readback.hwnd).map_err(to_mcp)?;
    let observed = observe_child_identity(identity.root_hwnd, identity.hwnd);
    revalidate_child_identity(&identity, observed.as_ref()).map_err(to_mcp)?;
    post_mouse_message(readback.hwnd, BM_CLICK, 0, Point { x: 0, y: 0 }).map_err(to_mcp)?;
    log_posted_click(params, readback, 1);
    Ok(())
}

#[cfg(not(windows))]
async fn post_point_clicks(
    _params: &ActClickParams,
    _point: Point,
    _root_hwnd: i64,
    _timing: DoubleClickTiming,
    prior_attempts: &[ActClickTierAttempt],
) -> Result<Option<PatternClickEvidence>, ErrorData> {
    Err(super::element::postmessage_failure(
        super::element::mark_click_not_dispatched(action_error_to_mcp(
            &synapse_action::ActionError::BackendUnavailable {
                detail: "act_click point HWND PostMessage is only available on Windows".to_owned(),
            },
        )),
        prior_attempts.to_vec(),
    ))
}

/// Posts the Win32 click sequence (`posted_click_steps`) at the readback's client
/// point. The cached child HWND is revalidated before every posted message, so a
/// child destroyed, reparented, or recycled during a hold is never clicked.
#[cfg(windows)]
pub(super) async fn post_hwnd_clicks(
    params: &ActClickParams,
    readback: &HwndMessageClickReadback,
    timing: DoubleClickTiming,
) -> Result<(), ErrorData> {
    let to_mcp = |error: synapse_action::ActionError| posted_click_error(&error, false);
    let (down, up, down_wparam) = mouse_button_messages(params.button).map_err(to_mcp)?;
    let double = double_click_message(down).map_err(to_mcp)?;
    let identity = snapshot_child_identity(readback.root_hwnd, readback.hwnd).map_err(to_mcp)?;
    let mut click_number = 0;
    post_hwnd_clicks_with(
        params,
        timing,
        class_accepts_double_clicks(readback.hwnd),
        || {
            let observed = observe_child_identity(identity.root_hwnd, identity.hwnd);
            revalidate_child_identity(&identity, observed.as_ref())
        },
        |step| {
            let (message, wparam) = match step {
                PostedMouseStep::Move => (WM_MOUSEMOVE, 0),
                PostedMouseStep::Down => (down, down_wparam),
                PostedMouseStep::DoubleClick => (double, down_wparam),
                PostedMouseStep::Up => (up, 0),
                PostedMouseStep::Hold | PostedMouseStep::InterClickDelay => return Ok(()),
            };
            post_mouse_message(readback.hwnd, message, wparam, readback.client_point)?;
            if step == PostedMouseStep::Up {
                click_number += 1;
                log_posted_click(params, readback, click_number);
            }
            Ok(())
        },
    )
    .await
}

/// Production posting loop, with OS operations injected. Identity failure and
/// a failed post both retain every successfully queued button.
#[cfg(windows)]
pub(super) async fn post_hwnd_clicks_with(
    params: &ActClickParams,
    timing: DoubleClickTiming,
    accepts_double: bool,
    mut revalidate: impl FnMut() -> Result<(), synapse_action::ActionError> + Send,
    mut post: impl FnMut(PostedMouseStep) -> Result<(), synapse_action::ActionError> + Send,
) -> Result<(), ErrorData> {
    let mut button_posted = false;
    for step in posted_click_steps(params.clicks, accepts_double) {
        match step {
            PostedMouseStep::Hold => {
                sleep(Duration::from_millis(u64::from(params.hold_ms))).await;
                continue;
            }
            PostedMouseStep::InterClickDelay => {
                sleep(Duration::from_millis(u64::from(
                    timing.inter_click_delay_ms,
                )))
                .await;
                continue;
            }
            _ => {}
        }
        revalidate().map_err(|error| posted_click_error(&error, button_posted))?;
        post(step).map_err(|error| posted_click_error(&error, button_posted))?;
        button_posted |= step != PostedMouseStep::Move;
    }
    Ok(())
}

/// #189: a PostMessage click that fails after a button message was posted has
/// already reached the target's queue (e.g. click 1 of 2 closed the child
/// window). Mark it delivered and stop the router so no tier replays it.
#[cfg(windows)]
fn posted_click_error(error: &synapse_action::ActionError, button_posted: bool) -> ErrorData {
    let error = action_error_to_mcp(error);
    if !button_posted {
        return super::element::mark_click_not_dispatched(error);
    }
    super::element::stop_click_router(
        crate::m2::postcondition::mark_input_delivered(error),
        "postmessage_partial_click_delivered",
    )
}

#[cfg(windows)]
fn log_posted_click(
    params: &ActClickParams,
    readback: &HwndMessageClickReadback,
    click_number: u32,
) {
    tracing::info!(
        code = "M2_ACT_CLICK_HWND_MESSAGE",
        kind = "act_click",
        root_hwnd = readback.root_hwnd,
        target_hwnd = readback.hwnd,
        target_class = %readback.class_name,
        client_x = readback.client_point.x,
        client_y = readback.client_point.y,
        click_number,
        button = ?params.button,
        "readback=window_message tool=act_click point_click_after"
    );
}

#[cfg(test)]
#[path = "tests/point_post_107.rs"]
mod tests_107;

#[cfg(test)]
#[path = "point_route_root_table_107c.rs"]
mod point_route_root_table_107c;

#[cfg(test)]
#[path = "tests/point_post_no_foreground_7.rs"]
mod tests_no_foreground_7;

#[cfg(test)]
#[path = "../tests/point_click_route_no_foreground_7.rs"]
mod point_click_route_no_foreground_7_tests;

#[cfg(all(test, windows))]
#[path = "tests/posted_delivery_189.rs"]
pub(crate) mod posted_delivery_189_tests;
