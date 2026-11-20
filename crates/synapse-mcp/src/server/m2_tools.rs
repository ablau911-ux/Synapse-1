#[path = "verified_click_router.rs"]
mod verified_click_router;
#[cfg(test)]
use verified_click_router::ClickTierPort;
use verified_click_router::{
    LiveClickTierPort, carry_click_delivery, click_delivery_snapshot, route_verified_click,
};

#[cfg(all(test, windows))]
#[path = "tests/related_foreground_probe_92.rs"]
mod synapse_a11y;

#[cfg(all(test, windows))]
#[path = "tests/related_background_handlers_92.rs"]
mod related_background_handlers_92;

use super::act_request_audit::ActRequestAudit;
use super::background_router::{
    BACKGROUND_FOREGROUND_SETTLE_ENV, NativeForegroundEvidence, NativeForegroundOutcome,
    background_click_foreground_error, background_foreground_settle_config_ms,
    background_foreground_settle_ms, background_guard_applies,
    foreground_read_blocks_background_action, native_foreground_section,
    posted_scroll_guard_window, settle_native_pattern_foreground,
};
use super::diagnostic_rate_limit_reset::schedule_reset;
use super::{
    ActClickParams, ActClickResponse, ActClipboardParams, ActClipboardResponse,
    ActFocusWindowParams, ActFocusWindowResponse, ActKeymapParams, ActKeymapResponse, ActPadParams,
    ActPadResponse, ActPressParams, ActPressResponse, ActScrollParams, ActScrollResponse,
    ActSetValueParams, ActSetValueResponse, ActStrokeParams, ActStrokeResponse, ActTypeParams,
    ActTypeResponse, ErrorData, Json, Parameters, ReleaseAllParams, ReleaseAllResponse,
    SessionTarget, SynapseService, act_click_with_handle_and_lease, act_clipboard_session_buffer,
    act_focus_window, act_focus_window_request_details, act_focus_window_target_hwnd,
    act_pad_with_handle, act_press_with_handle, act_set_value, act_set_value_request_details,
    act_stroke_validation_failure_details, act_stroke_with_handle, act_type_with_handle,
    action_preflight::{ActionPreflightReadback, ForegroundProof},
    release_all_with_handles, tool, tool_router, validate_act_stroke_params,
};
use crate::m1::{FindParams, FindResult, FindResultKind, FindScope, mcp_error};
use crate::m2::postcondition::{
    ActPostcondition, delivered_no_observed_delta_error, hash_json as verify_hash_json,
    mark_input_delivered, no_observed_delta_error as source_no_observed_delta_error,
    postcondition_failed_error, postcondition_not_requested, postcondition_observed_delta,
    postcondition_target_window_closed,
};
use crate::m2::{
    ActClickPostcondition, ActClickTierAttempt, ActStrokePlan, CLICK_REASON_NO_OBSERVED_DELTA,
    ClaimedPointTarget, ForegroundClickPolicy, HwndKeyboardTargetState, PointClickRoute,
    PressBackend, ResolvedKeymapPress, TypeBackend, act_click_claimed_point_postmessage,
    act_keymap_response_from_press, act_press_cdp_target, act_press_normalized_labels,
    act_press_postmessage_target, act_stroke_cdp_target, act_stroke_error_details,
    act_stroke_request_details, action_from_foreground_type_params, action_from_press_params,
    action_from_type_params, attach_click_tier_attempts, claimed_background_point_possible,
    click_target_foreground_guard_hwnds, click_target_root_hwnd, click_tier_delivered,
    click_tier_failed, current_point_click_route, emitted_text, hwnd_keyboard_target_state,
    point_click_resolves_root, resolve_keymap_press, unclaimed_point_target_error,
};
#[cfg(test)]
use crate::m2::{CLICK_TIER_FOREGROUND, CLICK_TIER_POSTMESSAGE};
use crate::m3::episodes::hex_encode;
use rmcp::{RoleServer, model::ErrorCode, service::RequestContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use synapse_action::{
    ACTION_QUEUE_CAPACITY, ActionEmitterSnapshotHandle, ActionError, ActionHandle,
    ActionStateSnapshot, RecordingBackend, ResolvedBackend, TokenBucketSnapshot,
};
use synapse_core::{
    AccessibleNode, Action, AimCurve, Backend, ButtonAction, ElementId, FocusedElement,
    ForegroundContext, KeystrokeDynamics, KeystrokeNaturalParams, MouseButton, MouseTarget,
    PathPoint, PathSpec, Point, Rect, StrokeMotionModel, StrokeTiming, UiaPattern, VelocityProfile,
    error_codes,
};
use synapse_perception::ObservationInput;
use tokio_util::sync::CancellationToken;

const ACT_STROKE_FOREGROUND_MONITOR_INTERVAL_MS: u64 = 10;
const ACTION_DIAGNOSTIC_RATE_LIMIT_CONFIRM: &str = "force-real-rate-limit-for-fsv";
const ACTION_DIAGNOSTIC_QUEUE_FULL_CONFIRM: &str = "saturate-real-action-queue-for-fsv";
const ACTION_DIAGNOSTIC_MAX_TTL_MS: u64 = 10_000;
const ACTION_DIAGNOSTIC_MIN_TTL_MS: u64 = 100;
const ACTION_DIAGNOSTIC_MAX_QUEUE_BLOCKER_MS: u32 = 10_000;
const ACTION_DIAGNOSTIC_MIN_QUEUE_BLOCKER_MS: u32 = 250;
const ACTION_DIAGNOSTIC_QUEUE_SETTLE_MS: u64 = 50;
const ACT_TYPE_BROWSER_URL_SOURCE_OF_TRUTH: &str = "browser_target.url";
const ACT_TYPE_BROWSER_URL_TEXT_INTEGRITY: &str = "browser_target_url_readback";
const ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH: &str = "foreground_text_readback";
const ACT_TYPE_TEXT_INTEGRITY_PREFIX: &str = "verify_delta_text_readback";
const ACT_TYPE_TEXT_SOURCE_UIA_VALUE: &str = "uia_focused_value";
const ACT_TYPE_TEXT_SOURCE_UIA_EMPTY: &str = "uia_focused_empty_value_or_text";
const ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE: &str = "cdp_active_element_value";
const ACT_TYPE_TEXT_SOURCE_OCR_FOCUSED_RECT: &str = "ocr_focused_rect_text";
const ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_SOURCE_OF_TRUTH: &str =
    "chrome_bridge_active_element.value";
const ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_TEXT_INTEGRITY: &str =
    "chrome_bridge_active_element_value_readback";
const ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_TIER: &str = "chrome_bridge_active_element";
const ACT_TYPE_FOREGROUND_FALLBACK_CLICK_HOLD_MS: u32 = 120;
const ACT_TYPE_FOREGROUND_FALLBACK_CLICK_DURATION_MS: u32 = 50;
const ACT_TYPE_VERIFY_POLL_INTERVAL_MS: u64 = 50;
const BACKGROUND_FOREGROUND_SOURCE_OF_TRUTH: &str = "os_foreground_window";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClickBeforeCapture {
    None,
    CoordinatePixels { hwnd: i64 },
    Semantic { target_window_hwnd: Option<i64> },
}

fn click_before_capture(
    params: &ActClickParams,
    target_window_hwnd: Option<i64>,
) -> ClickBeforeCapture {
    if !params.verify_delta {
        return ClickBeforeCapture::None;
    }
    if matches!(params.target, crate::m2::ActClickTarget::Point(_))
        && let Some(hwnd) = target_window_hwnd
    {
        return ClickBeforeCapture::CoordinatePixels { hwnd };
    }
    ClickBeforeCapture::Semantic { target_window_hwnd }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActPressBeforeCapture {
    None,
    NativeForeground,
    Semantic,
}

pub(super) fn act_press_before_capture(params: &ActPressParams) -> ActPressBeforeCapture {
    if params.verify_delta {
        ActPressBeforeCapture::Semantic
    } else if params.force_foreground_key_delivery {
        ActPressBeforeCapture::NativeForeground
    } else {
        ActPressBeforeCapture::None
    }
}

async fn act_type_fallback_actionable_click(
    handle: ActionHandle,
    element_id: &ElementId,
    target: &ActTypeForegroundFallbackTarget,
    actionable: impl FnOnce(
        &ElementId,
        i64,
        &[UiaPattern],
        &mut Rect,
    ) -> Result<Point, foreground_click_actionability::ClickPointRefusal>,
) -> Result<ActTypeForegroundFallbackTarget, ErrorData> {
    let mut target = target.clone();
    actionable(
        element_id,
        target.root_hwnd,
        &target.patterns,
        &mut target.bbox,
    )
    .map_err(|refusal| act_type_foreground_fallback_actionability_error(&target, &refusal))?;
    SynapseService::click_act_type_foreground_fallback_target(handle, &target).await?;
    Ok(target)
}

fn fill_related_window_evidence(
    evidence: &mut RelatedWindowEvidence,
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
) {
    evidence.owner_chain = related_window_owner_chain(after.foreground_hwnd);
    evidence.descendant_pids.clear();
    if after.foreground_pid != 0
        && after.foreground_pid != before.foreground_pid
        && !evidence.prior_windows.contains(&after.foreground_hwnd)
    {
        evidence.descendant_pids =
            crate::m2::process_descendants::live_descendant_pids(before.foreground_pid);
    }
}

fn related_action_result_allows_readback<T>(result: &Result<T, ErrorData>) -> bool {
    // Reconcile failed foreground verification without rechecking an accepted result.
    result.as_ref().err().is_some_and(|error| {
        matches!(
            click_error_data_code(error),
            Some(error_codes::ACTION_FOREGROUND_LOST) | Some(error_codes::ACTION_NO_OBSERVED_DELTA)
        )
    })
}

fn reconciled_related_click_response(
    params: &ActClickParams,
    postcondition: ActClickPostcondition,
    error: &ErrorData,
    started: Instant,
) -> ActClickResponse {
    let mut tier_attempts = click_tier_attempts_from_error(error);
    let reconciled = reconciled_click_attempt(&tier_attempts, error);
    let tier = reconciled.tier.clone();
    tier_attempts.push(reconciled);
    let timing = synapse_action::cached_double_click_timing();
    ActClickResponse {
        ok: true,
        used_invoke_pattern: tier == "uia",
        backend_used: tier.clone(),
        backend_tier_used: tier,
        required_foreground: tier_attempts
            .iter()
            .any(|attempt| attempt.required_foreground),
        tier_attempts,
        postcondition,
        press_hold_ms: params.hold_ms,
        double_click_window_ms: timing.window_ms,
        inter_click_delay_ms: timing.inter_click_delay_ms,
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionDiagnosticRateLimitOverrideParams {
    #[serde(default)]
    pub confirm: String,
    #[serde(default = "default_diagnostic_ttl_ms")]
    #[schemars(default = "default_diagnostic_ttl_ms", range(min = 100, max = 10000))]
    pub ttl_ms: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionDiagnosticRateLimitOverrideResponse {
    pub backend: String,
    pub ttl_ms: u64,
    pub before: TokenBucketReadback,
    pub after: TokenBucketReadback,
    pub reset_scheduled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionDiagnosticQueueFullSetupParams {
    #[serde(default)]
    pub confirm: String,
    #[serde(default = "default_queue_blocker_duration_ms")]
    #[schemars(
        default = "default_queue_blocker_duration_ms",
        range(min = 250, max = 10000)
    )]
    pub blocker_duration_ms: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionDiagnosticQueueFullSetupResponse {
    pub backend: String,
    pub expected_queue_capacity: u32,
    pub blocker_duration_ms: u32,
    pub blocker_from: Point,
    pub blocker_to: Point,
    pub blocker_queued: bool,
    pub filler_attempts: u32,
    pub queued_fillers: u32,
    pub queue_full_observed: bool,
    pub next_act_stroke_expected_error: String,
}

#[derive(Copy, Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenBucketReadback {
    pub capacity: u32,
    pub tokens: u32,
    pub refill_rate_per_s: u32,
    pub last_refill_ns: u64,
}

const fn default_diagnostic_ttl_ms() -> u64 {
    5_000
}

const fn default_queue_blocker_duration_ms() -> u32 {
    5_000
}

/// Unwrap an `Ok`, or - once the action's "started" audit row exists - write
/// the closing error row and return the error, so an early return never leaves
/// an orphan "started" row in `CF_ACTION_LOG` (#198).
macro_rules! audit_try {
    ($service:expr, $tool:expr, $request_context:expr, $result:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => {
                return audit_target_claim_denial($service, $tool, error, $request_context);
            }
        }
    };
}

impl SynapseService {
    /// Bind authorization and native delivery to one checked root (#62).
    fn dispatch_explicit_press_for_session_with(
        &self,
        requested_hwnd: i64,
        session_id: Option<&str>,
        mut resolve_root: impl FnMut(i64) -> Result<i64, ErrorData>,
        deliver: impl FnOnce(i64) -> Result<(), ErrorData>,
    ) -> Result<(), ErrorData> {
        let invalid = || {
            mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "act_press target root changed or disappeared before delivery; no input was sent",
            )
        };
        let root = resolve_root(requested_hwnd).map_err(|_| invalid())?;
        if root == 0 {
            return Err(invalid());
        }
        if let Some(session_id) = session_id {
            let target = SessionTarget::Window { hwnd: root };
            self.ensure_target_claim_allows_session("act_press", session_id, &target)?;
            if self
                .target_claim_for_session(session_id, &target)?
                .is_none()
            {
                return Err(mcp_error(
                    error_codes::TARGET_CLAIM_NOT_FOUND,
                    "act_press refused before delivery: explicit input target is not claimed by this MCP session; no input was sent",
                ));
            }
        }
        if resolve_root(requested_hwnd).map_err(|_| invalid())? != root {
            return Err(invalid());
        }
        deliver(root)
    }

    pub(super) fn ensure_act_stroke_claims_allow(
        &self,
        session_id: &str,
        route: &ActStrokeRoute,
        plan: &ActStrokePlan,
        root_at: impl Fn(Point) -> Option<i64>,
    ) -> Result<(), ErrorData> {
        let mut targets = stroke_claim_targets(route, plan, root_at);
        if targets.is_empty() {
            let target = match self.session_target(Some(session_id))? {
                Some(target) => target,
                None => super::target_claims::current_foreground_session_target()?,
            };
            targets.push(target);
        }
        targets.iter().try_for_each(|target| {
            self.ensure_target_claim_allows_session("act_stroke", session_id, target)
        })
    }

    fn capture_related_action_before(
        &self,
        target_hwnd: i64,
    ) -> Result<Option<(ClickDeltaSignature, RelatedWindowEvidence)>, ErrorData> {
        // Related-window evidence is optional for a background or locked desktop.
        let foreground = synapse_a11y::current_foreground_context().ok();
        if !foreground.is_some_and(|context| context.hwnd == target_hwnd) {
            return Ok(None);
        }
        let before = self.capture_coordinate_click_delta_signature(target_hwnd)?;
        Ok(Some((
            before,
            RelatedWindowEvidence {
                prior_windows: related_window_inventory()?,
                owner_chain: Vec::new(),
                descendant_pids: Vec::new(),
            },
        )))
    }

    fn related_action_postcondition(
        &self,
        tool: &str,
        observation: Option<(ClickDeltaSignature, RelatedWindowEvidence)>,
        timeout_ms: u32,
    ) -> Result<Option<ActPostcondition>, ErrorData> {
        let Some((before, mut evidence)) = observation else {
            return Ok(None);
        };
        let after = self.capture_native_foreground_signature()?;
        fill_related_window_evidence(&mut evidence, &before, &after);
        if related_window_relationship(&before, &after, Some(&evidence)).is_none() {
            return Ok(None);
        }
        let mut policy = ForegroundChangePolicy::reject();
        policy.related_window = Some(evidence);
        verify_foreground_transition(
            tool,
            "foreground_related_window",
            timeout_ms,
            signature_hash(&before)?,
            signature_hash(&after)?,
            &before,
            &after,
            &policy,
        )
        .map(Some)
    }
}

/// The reply of a foreground-guarded action: the action's own fields plus the
/// optional foreground evidence. schemars opens every flattened part, so the
/// wrapper itself must close the published output schema (#2399 G1).
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ForegroundGuardedResponse<T> {
    #[serde(flatten)]
    action: T,
    #[serde(flatten)]
    foreground: Option<NativeForegroundEvidence>,
}

impl<T> From<T> for ForegroundGuardedResponse<T> {
    fn from(action: T) -> Self {
        Self {
            action,
            foreground: None,
        }
    }
}

impl<T> std::ops::Deref for ForegroundGuardedResponse<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.action
    }
}

pub type ActClickToolResponse = ForegroundGuardedResponse<ActClickResponse>;

#[tool_router(router = m2_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Click a screen coordinate or UI Automation element. Default element delivery uses background UIA control patterns (Invoke, Toggle, SelectionItem, ExpandCollapse, LegacyIAccessible.DoDefaultAction). For CDP web element targets, auto_wait=true scrolls into view and polls actionability before dispatch; timeout returns BROWSER_WAIT_TIMEOUT with precise unmet predicates. When element coordinate delivery is needed, Synapse tries a background HWND PostMessage click to the resolved child window before escalating to the leased foreground coordinate tier; enabled keyboard-focusable edit/document/text or Value/Text targets bypass PostMessage and use the leased foreground coordinate tier so the real caret/focus state is placed. verify_delta reads the target window SoT for element clicks. coordinate_fallback_on_unsupported=true allows bbox-center coordinate delivery only for enabled keyboard-focusable edit/document/text targets or elements exposing Value/Text patterns; set false to fail closed with ACTION_ELEMENT_PATTERN_UNSUPPORTED. This mouse click tool does not synthesize WM_CHAR/dead-key keyboard text; use act_type/act_set_value for text. velocity_profile controls coordinate-move timing only, while explicit spatial paths belong to act_stroke. If a previously observed transient element expired before dispatch, returns TRANSIENT_ELEMENT_EXPIRED with re-observe/find guidance."
    )]
    pub async fn act_click(
        &self,
        params: Parameters<ActClickParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActClickToolResponse>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_click",
            "tool.invocation kind=act_click"
        );
        let background_proven = self.claimed_background_point_click(&params, &request_context);
        let preflight = match self.ensure_supported_use_allows_action_scoped(
            "act_click",
            click_auto_wait_element_id(&params),
            background_proven,
        ) {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_for_request("act_click", &error, &request_context);
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_click",
            &ActRequestAudit::started_details(ActRequestAudit::click(&params), &preflight),
            &request_context,
        )?;
        if let Err(error) = ensure_everquest_click_backend(&params, &preflight) {
            let result: Result<ActClickToolResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_click", &result, &request_context)?;
            return result.map(Json);
        }
        if let Err(error) = self.ensure_target_claim_allows_action(
            "act_click",
            click_claim_target(&params),
            &request_context,
        ) {
            return audit_target_claim_denial(self, "act_click", error, &request_context);
        }
        if let Err(error) = preflight_background_webview_uia_click(&params) {
            let result: Result<ActClickToolResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_click", &result, &request_context)?;
            return result.map(Json);
        }
        if let Err(error) = maybe_auto_wait_for_actionability(
            self,
            "act_click",
            &request_context,
            params.auto_wait,
            click_auto_wait_element_id(&params),
            params.auto_wait_timeout_ms,
            None,
            None,
            ActionabilityAutoWaitRequirement::Action,
        )
        .await
        {
            let result: Result<ActClickToolResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_click", &result, &request_context)?;
            return result.map(Json);
        }
        let foreground_guard = match act_click_target_foreground_guard(&params) {
            Ok(guard) => guard,
            Err(error) => {
                let result: Result<ActClickToolResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_click", &result, &request_context)?;
                return result.map(Json);
            }
        };
        // Keep click verification reads in the same native section as dispatch and settle.
        let foreground_section = BackgroundForegroundSection::enter(foreground_guard).await;
        // #107: the point route reads the root whenever the window is named;
        // the delta capture still reads it only under verify_delta.
        let route_root = if point_click_resolves_root(&params) {
            match click_target_root_hwnd(&params) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    let result: Result<ActClickToolResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_click", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let target_window_hwnd = route_root.filter(|_| params.verify_delta);
        let capture_route = click_before_capture(&params, target_window_hwnd);
        let before_delta_signature = if capture_route != ClickBeforeCapture::None {
            let capture = match capture_route {
                ClickBeforeCapture::CoordinatePixels { hwnd } => {
                    self.capture_coordinate_click_delta_signature(hwnd)
                }
                ClickBeforeCapture::Semantic { target_window_hwnd } => {
                    self.capture_click_delta_signature(160, target_window_hwnd)
                        .await
                }
                ClickBeforeCapture::None => {
                    self.capture_click_delta_signature(160, target_window_hwnd)
                        .await
                }
            };
            match capture {
                Ok(signature) => Some(signature),
                Err(error) => {
                    let result: Result<ActClickToolResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_click", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let verify_timeout_ms = params.verify_timeout_ms;
        let foreground_click_policy = audit_try!(
            self,
            "act_click",
            &request_context,
            self.foreground_click_policy_for_request(&params, &request_context)
        );
        let (handle, recording, _connection_closed_cancel) = audit_try!(
            self,
            "act_click",
            &request_context,
            self.m2_action_context_for_request(&request_context)
        );
        let handle = if recording.is_none() {
            let explicit_hwnd = audit_try!(
                self,
                "act_click",
                &request_context,
                click_target_root_hwnd(&params)
            );
            let target = audit_try!(
                self,
                "act_click",
                &request_context,
                self.resolve_input_delivery_target(explicit_hwnd, &handle)
            );
            handle.with_delivery_target(Some(target))
        } else {
            handle
        };
        let session_id = handle.session_id().unwrap_or("stdio").to_owned();
        // #107: a claimed point target whose window is not OS foreground is delivered
        // through its child-window tree (PostMessage) without SendInput. Route is
        // read from live foreground; a route that cannot run in background fails closed.
        // Only a live target_claim owned by this session authorizes PostMessage; a
        // caller-supplied verify_target_window_hwnd alone never does.
        let point_route = if recording.is_none() {
            let route = self
                .point_click_session_claim(&params, route_root, &request_context)
                .and_then(|claim| current_point_click_route(&params, route_root, claim));
            match route {
                Ok(route) => route,
                Err(error) => {
                    let result: Result<ActClickToolResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_click", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            PointClickRoute::Direct
        };
        let refusal = match point_route {
            PointClickRoute::RefuseForeground => Some(mcp_error(
                error_codes::FOREGROUND_ACTIVATION_REFUSED,
                "act_click refused before delivery: the claimed point target is not the OS foreground and this backend cannot deliver in the background; no input was sent. Use backend auto or software for background delivery",
            )),
            PointClickRoute::RefuseUnclaimed => Some(unclaimed_point_target_error(route_root)),
            PointClickRoute::Direct | PointClickRoute::PostMessage(_) => None,
        };
        if let Some(error) = refusal {
            let result: Result<ActClickToolResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_click", &result, &request_context)?;
            return result.map(Json);
        }
        let params_for_related_click = params.clone();
        let related_before = if recording.is_none()
            && matches!(point_route, PointClickRoute::Direct)
            && let Some(before) = before_delta_signature.as_ref()
        {
            audit_try!(
                self,
                "act_click",
                &request_context,
                self.capture_related_action_before(before.foreground_hwnd)
            )
        } else {
            None
        };
        let mut dispatched = None;
        let result = with_background_foreground_guard(
            "act_click",
            &session_id,
            foreground_section,
            "act_click.background_target",
            Box::pin(async {
                let started = Instant::now();
                let result = if let PointClickRoute::PostMessage(claim) = point_route {
                    match (&before_delta_signature, target_window_hwnd) {
                        (Some(_), Some(root_hwnd)) => {
                            async {
                                let Some(check) =
                                    click_effect::PointClickCheck::begin(self, &params, root_hwnd)?
                                else {
                                    return act_click_claimed_point_postmessage(
                                        &params,
                                        claim,
                                        Vec::new(),
                                    )
                                    .await;
                                };
                                let response =
                                    act_click_claimed_point_postmessage(&params, claim, Vec::new())
                                        .await?;
                                // #107 part d: the UI Automation pattern's own state read
                                // proved the click; otherwise the point check decides.
                                if response.postcondition.status == "verified_state" {
                                    return Ok(response);
                                }
                                let verification = check.finish(self, verify_timeout_ms).await;
                                verified_click_router::finish_verified_click(
                                    response,
                                    verification,
                                    false,
                                )
                            }
                            .await
                        }
                        _ => act_click_claimed_point_postmessage(&params, claim, Vec::new()).await,
                    }
                } else if let Some(before) = before_delta_signature {
                    self.act_click_with_verified_router(
                        handle,
                        recording,
                        params,
                        before,
                        verify_timeout_ms,
                        target_window_hwnd,
                        foreground_click_policy,
                        started,
                    )
                    .await
                } else {
                    act_click_with_handle_and_lease(
                        handle,
                        recording,
                        params,
                        foreground_click_policy,
                    )
                    .await
                };
                dispatched = Some(click_delivery_snapshot(&result));
                if related_action_result_allows_readback(&result) {
                    match self.related_action_postcondition(
                        "act_click",
                        related_before,
                        verify_timeout_ms,
                    ) {
                        Ok(Some(postcondition)) => match result {
                            Ok(mut response) => {
                                response.postcondition = postcondition;
                                Ok(response)
                            }
                            Err(error) => Ok(reconciled_related_click_response(
                                &params_for_related_click,
                                postcondition,
                                &error,
                                started,
                            )),
                        },
                        Ok(None) => result,
                        Err(error) => Err(error),
                    }
                } else {
                    result
                }
            }),
            |response| {
                if response.postcondition.related_window_observed.is_some() {
                    return None;
                }
                Some(BackgroundGuardReply {
                    required_foreground: &mut response.required_foreground,
                    source_of_truth: response.postcondition.source_of_truth.as_deref(),
                    backend_tier_used: &response.backend_tier_used,
                })
            },
        )
        .await;
        let carry_delivery = |error| match &dispatched {
            Some(dispatched) => carry_click_delivery(error, dispatched),
            None => error,
        };
        let result = result.map_err(&carry_delivery);
        self.audit_action_result_for_request("act_click", &result, &request_context)
            .map_err(carry_delivery)?;
        result.map(Json)
    }

    #[tool(
        description = "Type text. With into_element, routes through background CDP insertText for web nodes, foreground-safe native HWND text messages for UIA-resolved edit controls, UIA ValuePattern.SetValue with value readback for native elements without a native edit HWND, or a leased foreground click/type fallback for verified Chromium UIA editable targets when CDP is unavailable and the target window is already foreground. For CDP into_element targets, auto_wait=true scrolls into view and waits for editable actionability before typing; timeout returns BROWSER_WAIT_TIMEOUT with precise unmet predicates. Without into_element, types through the leased foreground keyboard backend."
    )]
    pub async fn act_type(
        &self,
        params: Parameters<ActTypeParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActTypeResponse>, ErrorData> {
        let mut params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_type",
            "tool.invocation kind=act_type"
        );
        let preflight = match self
            .ensure_supported_use_allows_action_for("act_type", params.into_element.as_ref())
        {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_for_request("act_type", &error, &request_context);
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_type",
            &ActRequestAudit::started_details(ActRequestAudit::type_text(&params), &preflight),
            &request_context,
        )?;
        if let Err(error) = self.ensure_target_claim_allows_action(
            "act_type",
            params.into_element.as_ref().and_then(element_claim_target),
            &request_context,
        ) {
            return audit_target_claim_denial(self, "act_type", error, &request_context);
        }
        if let Err(error) = maybe_auto_wait_for_actionability(
            self,
            "act_type",
            &request_context,
            params.auto_wait,
            params.into_element.as_ref(),
            params.auto_wait_timeout_ms,
            None,
            None,
            ActionabilityAutoWaitRequirement::Editable,
        )
        .await
        {
            let result: Result<ActTypeResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_type", &result, &request_context)?;
            return result.map(Json);
        }
        let browser_url_policy = match act_type_browser_url_policy(&params) {
            Ok(policy) => policy,
            Err(error) => {
                let result: Result<ActTypeResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_type", &result, &request_context)?;
                return result.map(Json);
            }
        };
        let visual_delta_target_window_hwnd =
            match act_type_visual_delta_target_window(&params, browser_url_policy.as_ref()) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    let result: Result<ActTypeResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
            };
        let session_id = audit_try!(
            self,
            "act_type",
            &request_context,
            super::context::mcp_session_id_from_request_context(&request_context)
        );
        let verify_timeout_ms = params.verify_timeout_ms;
        let emitted = emitted_text(&params);
        if let Some(session_id_for_target) = session_id.as_deref() {
            match self
                .act_type_chrome_bridge_session_target(session_id_for_target, &params, &emitted)
                .await
            {
                Ok(Some(response)) => {
                    let result: Result<ActTypeResponse, ErrorData> = Ok(response);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
                Ok(None) => {}
                Err(error) => {
                    let result: Result<ActTypeResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        }
        let foreground_fallback =
            match act_type_chromium_foreground_fallback_target(params.into_element.as_ref()) {
                Ok(target) => target,
                Err(error) => {
                    let result: Result<ActTypeResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
            };
        let requires_foreground_route =
            act_type_requires_foreground_route(&params, foreground_fallback.as_ref());
        let (handle, recording, _connection_closed_cancel) = audit_try!(
            self,
            "act_type",
            &request_context,
            self.m2_action_context_for_request(&request_context)
        );
        let (handle, type_delivery_hwnd) = if requires_foreground_route && recording.is_none() {
            let explicit_hwnd = params
                .verify_target_window_hwnd
                .or_else(|| foreground_fallback.as_ref().map(|target| target.root_hwnd))
                .or_else(|| {
                    params
                        .into_element
                        .as_ref()
                        .and_then(|id| id.parts().ok())
                        .map(|parts| parts.hwnd)
                });
            let session_hwnd = audit_try!(
                self,
                "act_type",
                &request_context,
                self.implicit_input_delivery_hwnd(&handle)
            )
            .map(|(hwnd, _source)| hwnd);
            let hwnd = audit_try!(
                self,
                "act_type",
                &request_context,
                crate::m2::resolve_type_delivery_target(
                    explicit_hwnd,
                    session_hwnd,
                    preflight_foreground_hwnd(&preflight),
                )
                .map_err(|error| crate::m2::action_error_to_mcp(&error))
            );
            let target = audit_try!(
                self,
                "act_type",
                &request_context,
                self.resolve_input_delivery_target(Some(hwnd), &handle)
            );
            let root_hwnd = target.root_hwnd;
            (handle.with_delivery_target(Some(target)), root_hwnd)
        } else {
            (handle, 0)
        };
        if requires_foreground_route
            && let Err(error) = self.ensure_act_type_foreground(&preflight, recording.as_ref())
        {
            let result: Result<ActTypeResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_type", &result, &request_context)?;
            return result.map(Json);
        }
        let mut claimed_type_route = false;
        let _lease_guard = if requires_foreground_route {
            match acquire_tool_foreground_input_lease(self, "act_type", &request_context).or_else(
                |error| {
                    if error.data.as_ref().and_then(|data| data.get("code"))
                        == Some(&serde_json::Value::String(
                            error_codes::TOOL_PROFILE_POLICY_DENIED.to_owned(),
                        ))
                    {
                        self.acquire_claimed_input_lease(
                            "act_type",
                            params.verify_target_window_hwnd,
                            synapse_action::DEFAULT_LEASE_TTL_MS,
                            &request_context,
                        )
                        .inspect(|_guard| {
                            claimed_type_route = true;
                        })
                    } else {
                        Err(error)
                    }
                },
            ) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    let result: Result<ActTypeResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let before_text_signature = if let Some(target) = foreground_fallback.as_ref() {
            let mut foreground_params = params.clone();
            foreground_params.into_element = None;
            if let Err(error) = action_from_foreground_type_params(&foreground_params) {
                let result: Result<ActTypeResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_type", &result, &request_context)?;
                return result.map(Json);
            }
            if let Err(error) = self.ensure_act_type_foreground_fallback_target(
                &preflight,
                target,
                recording.as_ref(),
            ) {
                let result: Result<ActTypeResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_type", &result, &request_context)?;
                return result.map(Json);
            }
            // #194: same actionability gate as act_set_field_text - scroll into view,
            // refresh the live bbox, and refuse a click point outside the target window
            // before any real-cursor click.
            let element_id = audit_try!(
                self,
                "act_type",
                &request_context,
                params.into_element.as_ref().ok_or_else(|| {
                    mcp_error(
                        error_codes::ACTION_TARGET_INVALID,
                        "foreground fallback requires an element",
                    )
                })
            );
            let actionable_target = match act_type_fallback_actionable_click(
                handle.clone(),
                element_id,
                target,
                foreground_click_actionability::actionable_click_point,
            )
            .await
            {
                Ok(target) => target,
                Err(error) => {
                    let result: Result<ActTypeResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
            };
            let target = &actionable_target;
            let focus_readback = match self
                .capture_act_type_text_signature(160, true, false, None)
                .await
            {
                Ok(signature) => signature,
                Err(error) => {
                    let result: Result<ActTypeResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
            };
            if let Err(error) =
                act_type_foreground_fallback_focus_matches_target(target, &focus_readback.signature)
            {
                let result: Result<ActTypeResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_type", &result, &request_context)?;
                return result.map(Json);
            }
            tracing::info!(
                code = "M2_ACT_TYPE_CHROMIUM_FOREGROUND_FALLBACK_READY",
                element_id = %target.element_id,
                root_hwnd = target.root_hwnd,
                role = %target.role,
                "readback=foreground_text tool=act_type into_element_fallback=chromium_uia_value_pattern_refused"
            );
            params = foreground_params;
            params.verify_delta.then_some(focus_readback)
        } else if act_type_should_capture_text_signature(&params) {
            match self
                .capture_act_type_text_signature(
                    160,
                    browser_url_policy.is_none() && visual_delta_target_window_hwnd.is_none(),
                    browser_url_policy.is_some(),
                    session_id.as_deref(),
                )
                .await
            {
                Ok(signature) => Some(signature),
                Err(error) => {
                    let result: Result<ActTypeResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_type", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let before_visual_signature =
            if act_type_should_capture_visual_signature(&params, visual_delta_target_window_hwnd) {
                match self
                    .capture_action_delta_signature(
                        160,
                        None,
                        false,
                        visual_delta_target_window_hwnd,
                    )
                    .await
                {
                    Ok(signature) => Some(signature),
                    Err(error) => {
                        let result: Result<ActTypeResponse, ErrorData> = Err(error);
                        self.audit_action_result_for_request(
                            "act_type",
                            &result,
                            &request_context,
                        )?;
                        return result.map(Json);
                    }
                }
            } else {
                None
            };
        if claimed_type_route
            && let Err(error) = self.ensure_claimed_input_target(
                "act_type",
                params.verify_target_window_hwnd,
                &request_context,
            )
        {
            let result: Result<ActTypeResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_type", &result, &request_context)?;
            return result.map(Json);
        }
        let related_before = if recording.is_none()
            && params.verify_delta
            && params.into_element.is_none()
            && browser_url_policy.is_none()
        {
            audit_try!(
                self,
                "act_type",
                &request_context,
                self.capture_related_action_before(type_delivery_hwnd)
            )
        } else {
            None
        };
        let result = act_type_with_handle(handle, recording, params, type_delivery_hwnd).await;
        let delivered_response = result.as_ref().ok().cloned();
        let result = match (result, before_text_signature) {
            (Ok(response), Some(before)) => match self
                .verify_act_type_response(
                    response.clone(),
                    before,
                    verify_timeout_ms,
                    &emitted,
                    browser_url_policy.as_ref(),
                    session_id.as_deref(),
                )
                .await
            {
                Ok(response) => Ok(response),
                Err(error) if act_type_error_allows_visual_delta(&error) => {
                    match before_visual_signature {
                        Some(before_visual) => {
                            self.verify_act_type_visual_delta_response(
                                response,
                                before_visual,
                                verify_timeout_ms,
                                visual_delta_target_window_hwnd,
                                &error,
                            )
                            .await
                        }
                        None => Err(error),
                    }
                }
                Err(error) => Err(error),
            },
            (other, _) => other,
        };
        let result = if related_action_result_allows_readback(&result) {
            match (
                delivered_response,
                audit_try!(
                    self,
                    "act_type",
                    &request_context,
                    self.related_action_postcondition(
                        "act_type",
                        related_before,
                        verify_timeout_ms
                    )
                ),
            ) {
                (Some(mut response), Some(postcondition)) => {
                    response.postcondition = postcondition;
                    // A related window is an observed effect, not typed-text readback.
                    response.target_readback_required = true;
                    response.target_text_integrity =
                        "dispatch_only_requires_target_readback".to_owned();
                    Ok(response)
                }
                _ => result,
            }
        } else {
            result
        };
        self.audit_action_result_for_request("act_type", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(
        description = "Set a UI Automation element's text/value without foreground. For known native edit HWNDs, routes through Win32 WM_SETTEXT; otherwise routes through UIA ValuePattern.SetValue. Both tiers require a separate target readback from the same Source of Truth and fail closed with probed tier details; there is no keyboard/foreground fallback."
    )]
    pub async fn act_set_value(
        &self,
        params: Parameters<ActSetValueParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ForegroundGuardedResponse<ActSetValueResponse>>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_set_value",
            "tool.invocation kind=act_set_value"
        );
        let request_details = act_set_value_request_details(&params);
        let preflight = match self.ensure_supported_use_allows_action("act_set_value") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_with_details_for_request(
                    "act_set_value",
                    &error,
                    &request_details,
                    &request_context,
                );
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_set_value",
            &json!({
                "request": request_details,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        if let Err(error) = self.ensure_target_claim_allows_action(
            "act_set_value",
            element_claim_target(&params.element_id),
            &request_context,
        ) {
            return audit_target_claim_denial(self, "act_set_value", error, &request_context);
        }
        let foreground_guard = match act_set_value_target_foreground_guard(&params.element_id) {
            Ok(guard) => guard,
            Err(error) => {
                let result: Result<ForegroundGuardedResponse<ActSetValueResponse>, ErrorData> =
                    Err(error);
                self.audit_action_result_for_request("act_set_value", &result, &request_context)?;
                return result.map(Json);
            }
        };
        let session_id = audit_try!(
            self,
            "act_set_value",
            &request_context,
            super::context::mcp_session_id_from_request_context(&request_context)
        );
        let foreground_section = BackgroundForegroundSection::enter(Some(foreground_guard)).await;
        let result = with_background_foreground_guard(
            "act_set_value",
            session_id.as_deref().unwrap_or("stdio"),
            foreground_section,
            "act_set_value.background_tier",
            act_set_value(params),
            |response| {
                Some(BackgroundGuardReply {
                    required_foreground: &mut response.required_foreground,
                    source_of_truth: Some(response.source_of_truth.as_str()),
                    backend_tier_used: &response.backend_tier_used,
                })
            },
        )
        .await;
        self.audit_action_result_for_request("act_set_value", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(
        description = "Set a field's text by REPLACING its full content — clear + type + verify in one call (the form-filling primitive; #882/#1299). Call with element_id, or with locator {window_hwnd?, role?, name?, name_substring?, automation_id?} so Synapse resolves a fresh UIA element at action time; when a locator-backed action hits A11Y_ELEMENT_STALE, Synapse re-resolves and retries once. CDP web element ids use a background select-all + insertText replace with a separate node-value readback; auto_wait=true scrolls CDP web nodes into view and waits for editable actionability before replacement. Chromium UIA editable targets (when CDP is unavailable) use the leased foreground tier — the target window must already be foreground (act_focus_window first), then click, Ctrl+A, type, and a separate UIA value readback; native elements route through the act_set_value background tiers. Empty text clears the field. Every tier fails closed with its own reason code — there is no cross-tier fallback and no append behavior."
    )]
    pub async fn act_set_field_text(
        &self,
        params: Parameters<crate::m2::ActSetFieldTextParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ForegroundGuardedResponse<crate::m2::ActSetFieldTextResponse>>, ErrorData>
    {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_set_field_text",
            "tool.invocation kind=act_set_field_text"
        );
        let request_details = crate::m2::act_set_field_text_request_details(&params);
        let preflight = match self.ensure_supported_use_allows_action("act_set_field_text") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_with_details_for_request(
                    "act_set_field_text",
                    &error,
                    &request_details,
                    &request_context,
                );
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_set_field_text",
            &json!({
                "request": request_details,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        if let Err(error) = crate::m2::validate_set_field_text_params(&params) {
            let result: Result<
                ForegroundGuardedResponse<crate::m2::ActSetFieldTextResponse>,
                ErrorData,
            > = Err(error);
            self.audit_action_result_for_request("act_set_field_text", &result, &request_context)?;
            return result.map(Json);
        }
        let params =
            match self.act_set_field_text_resolve_params(&params, &request_context, "initial") {
                Ok(params) => params,
                Err(error) => {
                    let result: Result<
                        ForegroundGuardedResponse<crate::m2::ActSetFieldTextResponse>,
                        ErrorData,
                    > = Err(error);
                    self.audit_action_result_for_request(
                        "act_set_field_text",
                        &result,
                        &request_context,
                    )?;
                    return result.map(Json);
                }
            };
        let element_id = match crate::m2::required_element_id(&params) {
            Ok(element_id) => element_id.clone(),
            Err(error) => {
                let result: Result<
                    ForegroundGuardedResponse<crate::m2::ActSetFieldTextResponse>,
                    ErrorData,
                > = Err(error);
                self.audit_action_result_for_request(
                    "act_set_field_text",
                    &result,
                    &request_context,
                )?;
                return result.map(Json);
            }
        };
        if let Err(error) = self.ensure_target_claim_allows_action(
            "act_set_field_text",
            element_claim_target(&element_id),
            &request_context,
        ) {
            return audit_target_claim_denial(self, "act_set_field_text", error, &request_context);
        }
        let result = self
            .act_set_field_text_execute_resolved(&params, &preflight, &request_context, "initial")
            .await;
        let result = if set_field_text_error_is_stale(&result) && params.locator.is_some() {
            tracing::warn!(
                code = "M2_ACT_SET_FIELD_TEXT_STALE_RETRY",
                element_id = %element_id,
                "act_set_field_text target went stale; re-resolving locator and retrying once"
            );
            match self.act_set_field_text_resolve_params(&params, &request_context, "stale_retry") {
                Ok(retry_params) => match crate::m2::required_element_id(&retry_params).cloned() {
                    Ok(retry_element_id) => {
                        if let Err(error) = self.ensure_target_claim_allows_action(
                            "act_set_field_text",
                            element_claim_target(&retry_element_id),
                            &request_context,
                        ) {
                            Err(error)
                        } else {
                            self.act_set_field_text_execute_resolved(
                                &retry_params,
                                &preflight,
                                &request_context,
                                "stale_retry",
                            )
                            .await
                        }
                    }
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            }
        } else {
            result
        };
        self.audit_action_result_for_request("act_set_field_text", &result, &request_context)?;
        result.map(Json)
    }

    async fn act_set_field_text_execute_resolved(
        &self,
        params: &crate::m2::ActSetFieldTextParams,
        preflight: &ActionPreflightReadback,
        request_context: &RequestContext<RoleServer>,
        resolution_phase: &'static str,
    ) -> Result<ForegroundGuardedResponse<crate::m2::ActSetFieldTextResponse>, ErrorData> {
        let element_id = crate::m2::required_element_id(params)?;
        maybe_auto_wait_for_actionability(
            self,
            "act_set_field_text",
            request_context,
            params.auto_wait,
            Some(element_id),
            params.auto_wait_timeout_ms,
            None,
            None,
            ActionabilityAutoWaitRequirement::Editable,
        )
        .await?;
        let route = match crate::m2::set_field_text_route(element_id) {
            Ok(route) => route,
            Err(error) => {
                return Err(error);
            }
        };
        tracing::debug!(
            code = "M2_ACT_SET_FIELD_TEXT_ROUTE_RESOLVED",
            element_id = %element_id,
            resolution_phase,
            "readback=act_set_field_text route resolved"
        );
        match route {
            #[cfg(windows)]
            crate::m2::SetFieldTextRoute::Web { backend_node_id } => {
                crate::m2::act_set_field_text_web(params, backend_node_id)
                    .await
                    .map(Into::into)
            }
            #[cfg(not(windows))]
            crate::m2::SetFieldTextRoute::Web { .. } => Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "act_set_field_text web tier is only available on Windows",
            )),
            crate::m2::SetFieldTextRoute::ChromiumForeground {
                root_hwnd,
                process_name,
                metadata,
            } => self
                .act_set_field_text_foreground_tier(
                    params,
                    root_hwnd,
                    &process_name,
                    metadata,
                    preflight,
                    request_context,
                )
                .await
                .map(Into::into),
            crate::m2::SetFieldTextRoute::NativeBackground => {
                Self::act_set_field_text_background_guarded(params, request_context).await
            }
        }
    }

    fn act_set_field_text_resolve_params(
        &self,
        params: &crate::m2::ActSetFieldTextParams,
        request_context: &RequestContext<RoleServer>,
        resolution_phase: &'static str,
    ) -> Result<crate::m2::ActSetFieldTextParams, ErrorData> {
        let Some(locator) = params.locator.as_ref() else {
            return if params.element_id.is_some() {
                Ok(params.clone())
            } else {
                Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "act_set_field_text requires element_id or locator",
                ))
            };
        };
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?;
        let target = self.session_target(session_id.as_deref())?;
        let window_hwnd = set_field_text_locator_window_hwnd(params, locator, target.as_ref())?;
        if let Some(session_id) = session_id.as_deref() {
            super::agent_scope::ensure_access(
                session_id,
                &SessionTarget::Window { hwnd: window_hwnd },
            )?;
        }
        let element_id = self.resolve_native_action_locator(
            "act_set_field_text",
            locator,
            window_hwnd,
            resolution_phase,
        )?;
        Ok(crate::m2::params_with_resolved_element(params, element_id))
    }

    pub(super) fn resolve_native_action_locator(
        &self,
        tool: &'static str,
        locator: &crate::m2::ActSetFieldTextLocator,
        window_hwnd: i64,
        resolution_phase: &'static str,
    ) -> Result<ElementId, ErrorData> {
        let find_params = set_field_text_locator_find_params(locator, window_hwnd);
        let state = crate::m1::FindStateSnapshot::from(&*self.m1_state()?);
        let mut input = crate::m1::build_find_input(&state, &find_params, Some(window_hwnd))?;
        self.populate_find_detection(&mut input)?;
        let response = crate::m1::match_find_input(&input, &find_params);
        let results = response
            .results
            .into_iter()
            .filter(set_field_text_find_result_is_element)
            .filter(|result| set_field_text_locator_exact_name_matches(locator, result))
            .collect::<Vec<_>>();
        match results.as_slice() {
            [result] => {
                let element_id = result.element_id.clone().ok_or_else(|| {
                    mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!("{tool} locator matched an element result without element_id"),
                    )
                })?;
                tracing::info!(
                    code = "M2_ACT_SET_FIELD_TEXT_LOCATOR_RESOLVED",
                    element_id = %element_id,
                    window_hwnd,
                    resolution_phase,
                    role = result.role.as_deref().unwrap_or(""),
                    automation_id_present = result.automation_id.is_some(),
                    tool,
                    "readback=locator outcome=resolved"
                );
                Ok(element_id)
            }
            [] => Err(set_field_text_locator_resolution_error(
                tool,
                locator,
                window_hwnd,
                resolution_phase,
                "not_found",
                results.as_slice(),
            )),
            many => Err(set_field_text_locator_resolution_error(
                tool,
                locator,
                window_hwnd,
                resolution_phase,
                "ambiguous",
                many,
            )),
        }
    }

    #[tool(
        description = "Operator-intent foreground action: lease-gated focus/activation of one visible top-level native window by exact hwnd, unique title_regex, or unique pid. Do not use as an action/perception precondition. It fails closed on missing, ambiguous, or contended targets and verifies success with a separate GetForegroundWindow readback."
    )]
    pub async fn act_focus_window(
        &self,
        params: Parameters<ActFocusWindowParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActFocusWindowResponse>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_focus_window",
            "tool.invocation kind=act_focus_window"
        );
        let request_details = act_focus_window_request_details(&params);
        let recovery_proven = self.focus_recovery_claim_proven(&params, &request_context);
        let preflight = match self.ensure_supported_use_allows_action_scoped(
            "act_focus_window",
            None,
            recovery_proven,
        ) {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_with_details_for_request(
                    "act_focus_window",
                    &error,
                    &request_details,
                    &request_context,
                );
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_focus_window",
            &json!({
                "request": request_details,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        let focus_claim_target = match act_focus_window_target_hwnd(&params) {
            Ok(hwnd) => super::target_claims::window_session_target(hwnd),
            Err(error) => {
                return audit_target_claim_denial(
                    self,
                    "act_focus_window",
                    error,
                    &request_context,
                );
            }
        };
        if let Err(error) = self.ensure_target_claim_allows_action(
            "act_focus_window",
            Some(focus_claim_target),
            &request_context,
        ) {
            return audit_target_claim_denial(self, "act_focus_window", error, &request_context);
        }
        let mut lease_guard =
            match acquire_tool_foreground_input_lease(self, "act_focus_window", &request_context) {
                Ok(guard) => guard,
                Err(error) => {
                    self.audit_action_error_with_details_for_request(
                        "act_focus_window",
                        &error,
                        &request_details,
                        &request_context,
                    )?;
                    return Err(error);
                }
            };
        lease_guard.disable_context_restore("act_focus_window_intentional_foreground_change");
        let result = act_focus_window(params, recovery_proven).await;
        if let Ok(response) = &result
            && response.ok
            && response.after_foreground.hwnd == response.matched_window.hwnd
        {
            let target = audit_try!(
                self,
                "act_focus_window",
                &request_context,
                synapse_action::InputDeliveryTarget::resolve_window(
                    response.matched_window.hwnd,
                    "act_focus_window",
                )
                .map_err(|error| crate::m2::action_error_to_mcp(&error))
            );
            let (handle, _, _) = audit_try!(
                self,
                "act_focus_window",
                &request_context,
                self.m2_action_context_for_request(&request_context)
            );
            audit_try!(
                self,
                "act_focus_window",
                &request_context,
                handle
                    .bind_session_delivery_target(target)
                    .map_err(|error| crate::m2::action_error_to_mcp(&error))
            );
        }
        self.audit_action_result_for_request("act_focus_window", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(
        description = "Press a keyboard key or ordered chord. With an active session target and backend=auto/software, Synapse first uses background delivery: CDP Input.dispatchKeyEvent for CDP targets or HWND PostMessage keyboard messages for window targets. auto_wait=true with auto_wait_element_id scrolls that CDP web node into view and waits for actionability before pressing. PostMessage delivery is accepted only after a separate target text/selection readback changes; ignored posted keys fail with ACTION_NO_OBSERVED_DELTA. backend=hardware, recording, no active target, or declared foreground-transition verification uses the leased foreground keyboard path."
    )]
    pub async fn act_press(
        &self,
        params: Parameters<ActPressParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActPressResponse>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_press",
            "tool.invocation kind=act_press"
        );
        super::context::maybe_force_panic_during_act("act_press");
        let preflight = match self.ensure_supported_use_allows_action("act_press") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_for_request("act_press", &error, &request_context);
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_press",
            &ActRequestAudit::started_details(ActRequestAudit::press(&params), &preflight),
            &request_context,
        )?;
        // #202: check where the keys really go (explicit target, else the
        // session target) and the auto-wait element's whole window.
        let claim_check = press_claim_targets(&params).and_then(|(dispatch, element_window)| {
            self.ensure_target_claim_allows_action("act_press", dispatch, &request_context)?;
            match element_window {
                Some(window) => self.ensure_target_claim_allows_action(
                    "act_press",
                    Some(window),
                    &request_context,
                ),
                None => Ok(()),
            }
        });
        if let Err(error) = claim_check {
            return audit_target_claim_denial(self, "act_press", error, &request_context);
        }
        let mut foreground_change_policy = match act_press_foreground_change_policy(&params) {
            Ok(policy) => policy,
            Err(error) => {
                let result: Result<ActPressResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_press", &result, &request_context)?;
                return result.map(Json);
            }
        };
        if let Err(error) = action_from_press_params(&params) {
            let result: Result<ActPressResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_press", &result, &request_context)?;
            return result.map(Json);
        }
        if let Err(error) = maybe_auto_wait_for_actionability(
            self,
            "act_press",
            &request_context,
            params.auto_wait,
            params.auto_wait_element_id.as_ref(),
            params.auto_wait_timeout_ms,
            params.window_hwnd,
            params.cdp_target_id.as_deref(),
            ActionabilityAutoWaitRequirement::Action,
        )
        .await
        {
            let result: Result<ActPressResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_press", &result, &request_context)?;
            return result.map(Json);
        }
        let (handle, recording, connection_closed_cancel) = audit_try!(
            self,
            "act_press",
            &request_context,
            self.m2_action_context_for_request(&request_context)
        );
        let claimed_window_hwnd = match self
            .try_act_press_background_target(params.clone(), recording.is_some(), &request_context)
            .await
        {
            Ok(BackgroundPress::Delivered(response)) => {
                let result = Ok(*response);
                self.audit_action_result_for_request("act_press", &result, &request_context)?;
                return result.map(Json);
            }
            Ok(BackgroundPress::NotTaken) => None,
            Ok(BackgroundPress::ClaimedKeyPress { window_hwnd }) => Some(window_hwnd),
            Err(error) => {
                let result: Result<ActPressResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_press", &result, &request_context)?;
                return result.map(Json);
            }
        };
        let capture_route = act_press_before_capture(&params);
        let native_window_readback = capture_route == ActPressBeforeCapture::NativeForeground;
        let before_delta_signature = if capture_route != ActPressBeforeCapture::None {
            let captured = if native_window_readback {
                self.capture_native_foreground_signature()
            } else {
                self.capture_action_delta_signature(160, None, false, None)
                    .await
            };
            match captured {
                Ok(signature) => Some(signature),
                Err(error) => {
                    let result: Result<ActPressResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_press", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let verify_timeout_ms = params.verify_timeout_ms;
        let lease_ttl_ms = lease_ttl_for_hold_ms(params.hold_ms);
        let mut claimed_key_press = false;
        let _lease_guard = match acquire_tool_foreground_input_lease_with_ttl(
            self,
            "act_press",
            &request_context,
            lease_ttl_ms,
        )
        .or_else(|error| {
            // #2340 part B: like act_type's claimed route, but only for a chord
            // the key route sent here because the claimed window is in front.
            if claimed_window_hwnd.is_some()
                && error.data.as_ref().and_then(|data| data.get("code"))
                    == Some(&serde_json::Value::String(
                        error_codes::TOOL_PROFILE_POLICY_DENIED.to_owned(),
                    ))
            {
                self.acquire_claimed_input_lease(
                    "act_press",
                    claimed_window_hwnd,
                    lease_ttl_ms,
                    &request_context,
                )
                .inspect(|_guard| {
                    claimed_key_press = true;
                })
            } else {
                Err(error)
            }
        }) {
            Ok(guard) => guard,
            Err(error) => {
                let result: Result<ActPressResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_press", &result, &request_context)?;
                return result.map(Json);
            }
        };
        if before_delta_signature.is_some()
            && recording.is_none()
            && !foreground_change_policy.allow
        {
            // Snapshot the top-level HWNDs VISIBLE before delivery. A hidden owned
            // dialog shown by the chord (Notepad++ Find) is the chord's effect; the
            // same-process owner chain, not visibility, excludes foreign windows.
            let prior_windows = audit_try!(
                self,
                "act_press",
                &request_context,
                related_window_inventory()
            );
            foreground_change_policy.related_window = Some(RelatedWindowEvidence {
                prior_windows,
                owner_chain: Vec::new(),
                descendant_pids: Vec::new(),
            });
        }
        if claimed_key_press
            && let Err(error) =
                self.ensure_claimed_input_target("act_press", claimed_window_hwnd, &request_context)
        {
            let result: Result<ActPressResponse, ErrorData> = Err(error);
            self.audit_action_result_for_request("act_press", &result, &request_context)?;
            return result.map(Json);
        }
        let handle = if recording.is_none() {
            let target = audit_try!(
                self,
                "act_press",
                &request_context,
                self.resolve_input_delivery_target(params.window_hwnd, &handle)
            );
            handle.with_delivery_target(Some(target))
        } else {
            handle
        };
        let result =
            act_press_with_handle(handle, recording, connection_closed_cancel, params).await;
        let result = match (result, before_delta_signature) {
            (Ok(response), Some(before)) => {
                self.verify_act_press_response(
                    response,
                    before,
                    verify_timeout_ms,
                    foreground_change_policy,
                    native_window_readback,
                )
                .await
            }
            (other, _) => other,
        };
        self.audit_action_result_for_request("act_press", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(
        description = "Press a keyboard alias from the active profile keymap. With an active session target and backend=auto/software, resolves the alias before any lease and routes through the same background CDP/PostMessage keyboard tiers as act_press; hardware, recording, or no active target uses the leased foreground keyboard path."
    )]
    pub async fn act_keymap(
        &self,
        params: Parameters<ActKeymapParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActKeymapResponse>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_keymap",
            alias = %params.alias,
            "tool.invocation kind=act_keymap"
        );
        let request_details = json!({
            "alias": params.alias.trim(),
            "hold_ms": params.hold_ms,
            "backend": params.backend,
        });
        let preflight = match self.ensure_supported_use_allows_action("act_keymap") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_with_details_for_request(
                    "act_keymap",
                    &error,
                    &request_details,
                    &request_context,
                );
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_keymap",
            &json!({
                "request": request_details,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        if let Err(error) =
            self.ensure_target_claim_allows_action("act_keymap", None, &request_context)
        {
            return audit_target_claim_denial(self, "act_keymap", error, &request_context);
        }
        let load_active_profile = || -> Result<_, ErrorData> {
            let runtime = self.profile_runtime()?;
            let active_profile_id = runtime
                .active_profile_id()
                .map_err(|error| mcp_error(error.code(), error.to_string()))?
                .ok_or_else(|| {
                    mcp_error(
                        error_codes::PROFILE_NOT_FOUND,
                        "act_keymap requires an active profile",
                    )
                })?;
            runtime
                .profile(&active_profile_id)
                .map_err(|error| mcp_error(error.code(), error.to_string()))?
                .ok_or_else(|| {
                    mcp_error(
                        error_codes::PROFILE_NOT_FOUND,
                        format!("active profile {active_profile_id} was not found"),
                    )
                })
        };
        let profile = audit_try!(self, "act_keymap", &request_context, load_active_profile());
        let resolved = match resolve_keymap_press(&profile, &params) {
            Ok(resolved) => resolved,
            Err(error) => {
                let result: Result<ActKeymapResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_keymap", &result, &request_context)?;
                return result.map(Json);
            }
        };
        let (handle, recording, connection_closed_cancel) = audit_try!(
            self,
            "act_keymap",
            &request_context,
            self.m2_action_context_for_request(&request_context)
        );
        match self
            .try_act_keymap_background_target(&resolved, recording.is_some(), &request_context)
            .await
        {
            Ok(Some(response)) => {
                let result = Ok(response);
                self.audit_action_result_for_request("act_keymap", &result, &request_context)?;
                return result.map(Json);
            }
            Ok(None) => {}
            Err(error) => {
                let result: Result<ActKeymapResponse, ErrorData> = Err(error);
                self.audit_action_result_for_request("act_keymap", &result, &request_context)?;
                return result.map(Json);
            }
        }
        let _lease_guard = match acquire_tool_foreground_input_lease_with_ttl(
            self,
            "act_keymap",
            &request_context,
            lease_ttl_for_hold_ms(params.hold_ms),
        ) {
            Ok(guard) => guard,
            Err(error) => {
                self.audit_action_error_with_details_for_request(
                    "act_keymap",
                    &error,
                    &request_details,
                    &request_context,
                )?;
                return Err(error);
            }
        };
        let handle = if recording.is_none() {
            let target = audit_try!(
                self,
                "act_keymap",
                &request_context,
                self.resolve_input_delivery_target(resolved.press.window_hwnd, &handle)
            );
            handle.with_delivery_target(Some(target))
        } else {
            handle
        };
        let result = act_press_with_handle(
            handle,
            recording,
            connection_closed_cancel,
            resolved.press.clone(),
        )
        .await
        .map(|response| act_keymap_response_from_press(&resolved, response));
        self.audit_action_result_for_request("act_keymap", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(
        description = "Move, aim, or drag to a point/element target or along an explicit spatial path using timed continuous mouse samples; button unset moves/aims and button set drags; motion_model defaults to path and can use wind_mouse for point-to-point line strokes"
    )]
    pub async fn act_stroke(
        &self,
        params: Parameters<ActStrokeParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActStrokeResponse>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_stroke",
            "tool.invocation kind=act_stroke"
        );
        let plan = match validate_act_stroke_params(&params) {
            Ok(plan) => plan,
            Err(error) => {
                let failure_details = act_stroke_validation_failure_details(&params, &error);
                log_act_stroke_failure(&failure_details, &error);
                self.audit_action_error_with_details_for_request(
                    "act_stroke",
                    &error,
                    &failure_details,
                    &request_context,
                )?;
                return Err(error);
            }
        };
        let stroke_details = act_stroke_request_details(&params, &plan);
        let preflight = match self.ensure_supported_use_allows_action("act_stroke") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_with_details_for_request(
                    "act_stroke",
                    &error,
                    &json!({
                        "stroke": stroke_details,
                        "failure": act_stroke_error_details(&error),
                    }),
                    &request_context,
                );
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_stroke",
            &act_stroke_audit_details(&stroke_details, &preflight),
            &request_context,
        )?;
        let verify_timeout_ms = params.verify_timeout_ms;
        let (handle, recording, _connection_closed_cancel) = audit_try!(
            self,
            "act_stroke",
            &request_context,
            self.m2_action_context_for_request(&request_context)
        );
        // #202: the route is decided once; the claim check covers exactly what
        // that route touches (the session tab for the CDP route, else every
        // window, or the aim element's tab, under the planned path).
        let route = audit_try!(
            self,
            "act_stroke",
            &request_context,
            self.act_stroke_route_for_request(
                &params,
                &plan,
                recording.is_some(),
                &request_context,
            )
        );
        let session_id = audit_try!(
            self,
            "act_stroke",
            &request_context,
            super::context::mcp_session_id_from_request_context(&request_context)
        );
        let claim_check = session_id.as_deref().map_or(Ok(()), |session_id| {
            self.ensure_act_stroke_claims_allow(
                session_id,
                &route,
                &plan,
                crate::m2::root_hwnd_at_point,
            )
        });
        if let Err(error) = claim_check {
            return audit_target_claim_denial(self, "act_stroke", error, &request_context);
        }
        match self
            .try_act_stroke_cdp_background_target(&route, &params, &plan)
            .await
        {
            Ok(Some(response)) => {
                self.audit_action_ok_with_details_for_request(
                    "act_stroke",
                    &json!({
                        "response": response,
                        "stroke": stroke_details,
                        "preflight": preflight,
                    }),
                    &request_context,
                )?;
                return Ok(Json(response));
            }
            Ok(None) => {}
            Err(error) => {
                let failure_details =
                    act_stroke_failure_audit_details(&stroke_details, &preflight, &error);
                log_act_stroke_failure(&failure_details, &error);
                self.audit_action_error_with_details_for_request(
                    "act_stroke",
                    &error,
                    &failure_details,
                    &request_context,
                )?;
                return Err(error);
            }
        }
        let before_delta_signature = if params.verify_delta {
            match self.capture_raw_input_delta_signature(None, None, true) {
                Ok(signature) => Some(signature),
                Err(error) => {
                    self.audit_action_error_with_details_for_request(
                        "act_stroke",
                        &error,
                        &act_stroke_failure_audit_details(&stroke_details, &preflight, &error),
                        &request_context,
                    )?;
                    return Err(error);
                }
            }
        } else {
            None
        };
        let _lease_guard = if should_acquire_act_stroke_input_lease(
            recording.is_some(),
            plan.requires_input_lease(),
        ) {
            // #2350: without break_glass, real drag input is allowed only into
            // the session's own claimed window while it is in front (the rule
            // of claimed typing, #2340 part B).
            match acquire_tool_foreground_input_lease(self, "act_stroke", &request_context).or_else(
                |error| {
                    if error.data.as_ref().and_then(|data| data.get("code"))
                        != Some(&serde_json::Value::String(
                            error_codes::TOOL_PROFILE_POLICY_DENIED.to_owned(),
                        ))
                    {
                        return Err(error);
                    }
                    let claimed_hwnd = match session_id.as_deref() {
                        Some(session_id) => match self.session_target(Some(session_id))? {
                            Some(SessionTarget::Window { hwnd }) => Some(hwnd),
                            _ => None,
                        },
                        None => None,
                    };
                    self.acquire_claimed_input_lease(
                        "act_stroke",
                        claimed_hwnd,
                        synapse_action::DEFAULT_LEASE_TTL_MS,
                        &request_context,
                    )
                },
            ) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    let failure_details =
                        act_stroke_failure_audit_details(&stroke_details, &preflight, &error);
                    log_act_stroke_failure(&failure_details, &error);
                    self.audit_action_error_with_details_for_request(
                        "act_stroke",
                        &error,
                        &failure_details,
                        &request_context,
                    )?;
                    return Err(error);
                }
            }
        } else {
            None
        };
        let handle = if recording.is_none() {
            let explicit_hwnd = [&params.target, &params.to, &params.from]
                .into_iter()
                .flatten()
                .find_map(|target| match target {
                    crate::m2::ActStrokeTarget::Element(element) => {
                        element.element_id.parts().ok().map(|parts| parts.hwnd)
                    }
                    crate::m2::ActStrokeTarget::Point(_) => None,
                });
            let target = audit_try!(
                self,
                "act_stroke",
                &request_context,
                self.resolve_input_delivery_target(explicit_hwnd, &handle)
            );
            handle.with_delivery_target(Some(target))
        } else {
            handle
        };
        let foreground_monitor =
            should_monitor_act_stroke_foreground(recording.is_some(), plan.requires_input_lease())
                .then(|| self.start_act_stroke_foreground_monitor(&preflight));
        let result = act_stroke_with_handle(handle, recording, params.clone(), plan.clone()).await;
        let foreground_error = await_act_stroke_foreground_monitor(foreground_monitor).await;
        let result = match foreground_error {
            Some(error) => Err(error),
            None => result,
        };
        let result = match (result, before_delta_signature) {
            (Ok(response), Some(before)) => {
                self.verify_act_stroke_response(response, before, verify_timeout_ms)
                    .await
            }
            (other, _) => other,
        };
        match &result {
            Ok(response) => {
                self.audit_action_ok_with_details_for_request(
                    "act_stroke",
                    &json!({
                        "response": response,
                        "stroke": stroke_details,
                        "preflight": preflight,
                    }),
                    &request_context,
                )?;
            }
            Err(error) => {
                let failure_details =
                    act_stroke_failure_audit_details(&stroke_details, &preflight, error);
                log_act_stroke_failure(&failure_details, error);
                self.audit_action_error_with_details_for_request(
                    "act_stroke",
                    error,
                    &failure_details,
                    &request_context,
                )?;
            }
        }
        result.map(Json)
    }

    #[tool(
        description = "Scroll vertically or horizontally at the current pointer or screen point"
    )]
    pub async fn act_scroll(
        &self,
        params: Parameters<ActScrollParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ForegroundGuardedResponse<ActScrollResponse>>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_scroll",
            "tool.invocation kind=act_scroll"
        );
        let preflight = match self.ensure_supported_use_allows_action("act_scroll") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_for_request("act_scroll", &error, &request_context);
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_scroll",
            &ActRequestAudit::started_details(ActRequestAudit::scroll(&params), &preflight),
            &request_context,
        )?;
        let scroll_target = scroll_claim_target(&params);
        let checked_window_hwnd = match &scroll_target {
            Some(SessionTarget::Window { hwnd }) => Some(*hwnd),
            _ => None,
        };
        let claim_check = self
            .ensure_target_claim_allows_action(
                "act_scroll",
                scroll_target.clone(),
                &request_context,
            )
            .and_then(|()| {
                if params.at.is_some() && !params.uses_element_target() {
                    self.ensure_explicit_input_target_claim(
                        "act_scroll",
                        scroll_target.as_ref(),
                        &request_context,
                    )
                } else {
                    Ok(())
                }
            });
        if let Err(error) = claim_check {
            return audit_target_claim_denial(self, "act_scroll", error, &request_context);
        }
        // #191: both captures and dispatch stay bound to the root selected
        // for authorization. Dispatch rechecks the hit before each wheel post.
        let delta_region = (params.verify_delta && !params.uses_element_target()).then(|| {
            DeltaRegion::resolve(params.verify_delta_point_region(), |_| checked_window_hwnd)
        });
        let before_delta_signature = if let Some(region) = delta_region {
            match self.capture_raw_input_delta_signature(region.target_window, region.point, false)
            {
                Ok(signature) => Some(signature),
                Err(error) => {
                    let result: Result<ForegroundGuardedResponse<ActScrollResponse>, ErrorData> =
                        Err(error);
                    self.audit_action_result_for_request("act_scroll", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let verify_timeout_ms = params.verify_timeout_ms;
        let (handle, recording, _connection_closed_cancel) = audit_try!(
            self,
            "act_scroll",
            &request_context,
            self.m2_action_context_for_request(&request_context)
        );
        let foreground_guard = if params.target.as_ref().is_some_and(|target| {
            synapse_a11y::cdp_backend_from_element_id(&target.element_id).is_none()
        }) {
            match act_scroll_target_foreground_guard(&params) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    let result: Result<ForegroundGuardedResponse<ActScrollResponse>, ErrorData> =
                        Err(error);
                    self.audit_action_result_for_request("act_scroll", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            // #318c: a wheel posted at a point inside a claimed window is a
            // background action on that window and gets the same guard.
            posted_scroll_guard_window(
                params.uses_element_target(),
                params.at.is_some(),
                checked_window_hwnd,
            )
            .map(|root_hwnd| BackgroundTargetForegroundGuard {
                element_hwnd: root_hwnd,
                root_hwnd,
            })
        };
        let _lease_guard = if params.requires_input_lease() {
            match acquire_tool_foreground_input_lease(self, "act_scroll", &request_context) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    let result: Result<ForegroundGuardedResponse<ActScrollResponse>, ErrorData> =
                        Err(error);
                    self.audit_action_result_for_request("act_scroll", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let handle = if recording.is_none() {
            let target = audit_try!(
                self,
                "act_scroll",
                &request_context,
                self.resolve_input_delivery_target(checked_window_hwnd, &handle)
            );
            handle.with_delivery_target(Some(target))
        } else {
            handle
        };
        let session_id = handle.session_id().unwrap_or("stdio").to_owned();
        let foreground_section = BackgroundForegroundSection::enter(foreground_guard).await;
        let result = with_background_foreground_guard(
            "act_scroll",
            &session_id,
            foreground_section,
            "act_scroll.background_target",
            Box::pin(async {
                let result = crate::m2::act_scroll_with_handle_bound_target(
                    handle,
                    recording,
                    params,
                    checked_window_hwnd,
                )
                .await;
                match (result, before_delta_signature) {
                    (Ok(response), Some(before)) => {
                        self.verify_act_scroll_response(
                            response,
                            before,
                            verify_timeout_ms,
                            delta_region.unwrap_or_default(),
                        )
                        .await
                    }
                    (other, _) => other,
                }
            }),
            |response| {
                Some(BackgroundGuardReply {
                    required_foreground: &mut response.required_foreground,
                    source_of_truth: response.postcondition.source_of_truth.as_deref(),
                    backend_tier_used: &response.backend_tier_used,
                })
            },
        )
        .await;
        self.audit_action_result_for_request("act_scroll", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(description = "Apply a virtual gamepad report and optionally return it to neutral")]
    pub async fn act_pad(
        &self,
        params: Parameters<ActPadParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActPadResponse>, ErrorData> {
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_pad",
            "tool.invocation kind=act_pad"
        );
        let preflight = match self.ensure_supported_use_allows_action("act_pad") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_for_request("act_pad", &error, &request_context);
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            "act_pad",
            &ActRequestAudit::started_details(ActRequestAudit::pad(&params), &preflight),
            &request_context,
        )?;
        if let Err(error) =
            self.ensure_target_claim_allows_action("act_pad", None, &request_context)
        {
            return audit_target_claim_denial(self, "act_pad", error, &request_context);
        }
        let (handle, recording, _connection_closed_cancel) = audit_try!(
            self,
            "act_pad",
            &request_context,
            self.m2_action_context_for_request(&request_context)
        );
        let snapshot_handle = if params.verify_delta {
            Some(audit_try!(
                self,
                "act_pad",
                &request_context,
                self.m2_snapshot_handle()
            ))
        } else {
            None
        };
        let before_snapshot = if let Some(snapshot_handle) = &snapshot_handle {
            match snapshot_handle.snapshot().await {
                Ok(snapshot) => Some(snapshot),
                Err(error) => {
                    let error = mcp_error(error.code(), error.to_string());
                    let result: Result<ActPadResponse, ErrorData> = Err(error);
                    self.audit_action_result_for_request("act_pad", &result, &request_context)?;
                    return result.map(Json);
                }
            }
        } else {
            None
        };
        let verify_timeout_ms = params.verify_timeout_ms;
        let result = act_pad_with_handle(handle, recording, params).await;
        let result = match (result, before_snapshot, snapshot_handle) {
            (Ok(response), Some(before), Some(snapshot_handle)) => {
                self.verify_act_pad_response(response, before, snapshot_handle, verify_timeout_ms)
                    .await
            }
            (other, _, _) => other,
        };
        self.audit_action_result_for_request("act_pad", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(
        description = "Read, write, or clear this MCP session's virtual clipboard buffer. The default path is background-safe: it does not touch the real OS clipboard and does not acquire the foreground/input lease."
    )]
    pub async fn act_clipboard(
        &self,
        params: Parameters<ActClipboardParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActClipboardResponse>, ErrorData> {
        let params = params.0;
        let session_id = super::context::mcp_session_id_from_request_context(&request_context)?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::HTTP_SESSION_INVALID,
                    "act_clipboard requires an MCP session id for session-scoped virtual clipboard state",
                )
            })?;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_clipboard",
            "tool.invocation kind=act_clipboard"
        );
        let request_details = clipboard_request_audit_details(&params);
        self.audit_action_started_with_details_for_session(
            "act_clipboard",
            &request_details,
            &session_id,
        )?;
        let result =
            self.act_clipboard_for_session(params, &session_id, "session_clipboard_buffer");
        match &result {
            Ok(response) => {
                self.audit_action_ok_with_details_for_session(
                    "act_clipboard",
                    &clipboard_response_audit_details(response),
                    &session_id,
                )?;
            }
            Err(error) => {
                self.audit_action_error_with_details_for_session(
                    "act_clipboard",
                    error,
                    &request_details,
                    &session_id,
                )?;
            }
        }
        result.map(Json)
    }

    #[tool(
        description = "FSV diagnostic: temporarily force the real software action rate limiter empty so the next real act_stroke proves ACTION_RATE_LIMITED through the normal MCP action path"
    )]
    pub async fn action_diagnostic_rate_limit_override(
        &self,
        params: Parameters<ActionDiagnosticRateLimitOverrideParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActionDiagnosticRateLimitOverrideResponse>, ErrorData> {
        let params = params.0;
        const TOOL: &str = "action_diagnostic_rate_limit_override";
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = TOOL,
            "tool.invocation kind=action_diagnostic_rate_limit_override"
        );
        let request_details = json!({
            "backend": ResolvedBackend::Software.as_str(),
            "ttl_ms": params.ttl_ms,
        });
        if let Err(error) =
            require_diagnostic_confirm(&params.confirm, ACTION_DIAGNOSTIC_RATE_LIMIT_CONFIRM, TOOL)
        {
            self.audit_action_denied_with_details_for_request(
                TOOL,
                &error,
                &request_details,
                &request_context,
            );
            return Err(error);
        }
        if let Err(error) = validate_diagnostic_ttl_ms(params.ttl_ms) {
            self.audit_action_denied_with_details_for_request(
                TOOL,
                &error,
                &request_details,
                &request_context,
            );
            return Err(error);
        }
        let preflight = match self.ensure_supported_use_allows_action("act_stroke") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_with_details_for_request(
                    TOOL,
                    &error,
                    &request_details,
                    &request_context,
                );
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            TOOL,
            &json!({
                "request": request_details,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        let control = audit_try!(self, TOOL, &request_context, self.m2_rate_limit_control());
        let (override_readback, override_generation) = audit_try!(
            self,
            TOOL,
            &request_context,
            control
                .override_backend(ResolvedBackend::Software, 0, 0)
                .map_err(diagnostic_action_error_to_mcp)
        );
        let response = ActionDiagnosticRateLimitOverrideResponse {
            backend: override_readback.backend.as_str().to_owned(),
            ttl_ms: params.ttl_ms,
            before: token_bucket_readback(override_readback.before),
            after: token_bucket_readback(override_readback.after),
            reset_scheduled: true,
        };
        schedule_reset(
            control,
            ResolvedBackend::Software,
            params.ttl_ms,
            override_generation,
        );
        self.audit_action_ok_with_details_for_request(
            TOOL,
            &json!({
                "response": response,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        Ok(Json(response))
    }

    #[tool(
        description = "FSV diagnostic: saturate the real bounded action queue behind a long software blocker so the next real act_stroke proves ACTION_QUEUE_FULL through the normal MCP action path"
    )]
    pub async fn action_diagnostic_queue_full_setup(
        &self,
        params: Parameters<ActionDiagnosticQueueFullSetupParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActionDiagnosticQueueFullSetupResponse>, ErrorData> {
        let params = params.0;
        const TOOL: &str = "action_diagnostic_queue_full_setup";
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = TOOL,
            "tool.invocation kind=action_diagnostic_queue_full_setup"
        );
        let request_details = json!({
            "backend": ResolvedBackend::Software.as_str(),
            "expected_queue_capacity": ACTION_QUEUE_CAPACITY,
            "blocker_duration_ms": params.blocker_duration_ms,
        });
        if let Err(error) =
            require_diagnostic_confirm(&params.confirm, ACTION_DIAGNOSTIC_QUEUE_FULL_CONFIRM, TOOL)
        {
            self.audit_action_denied_with_details_for_request(
                TOOL,
                &error,
                &request_details,
                &request_context,
            );
            return Err(error);
        }
        if let Err(error) = validate_queue_blocker_duration_ms(params.blocker_duration_ms) {
            self.audit_action_denied_with_details_for_request(
                TOOL,
                &error,
                &request_details,
                &request_context,
            );
            return Err(error);
        }
        let preflight = match self.ensure_supported_use_allows_action("act_stroke") {
            Ok(preflight) => preflight,
            Err(error) => {
                self.audit_action_denied_with_details_for_request(
                    TOOL,
                    &error,
                    &request_details,
                    &request_context,
                );
                return Err(error);
            }
        };
        self.audit_action_started_with_details_for_request(
            TOOL,
            &json!({
                "request": request_details,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        // The blocker injects a real cursor stroke, so it takes the standard
        // foreground input lease (and break_glass gate) like every other real
        // input path (#62). The cursor is deliberately not restored: the
        // blocker is still running when this call returns.
        let mut _foreground_lease = audit_try!(
            self,
            TOOL,
            &request_context,
            acquire_tool_foreground_input_lease_with_ttl(
                self,
                TOOL,
                &request_context,
                lease_ttl_for_hold_ms(params.blocker_duration_ms),
            )
        );
        _foreground_lease.disable_context_restore("queue_full_blocker_still_running");
        let (handle, recording, _connection_closed_cancel) = audit_try!(
            self,
            TOOL,
            &request_context,
            self.m2_action_context_for_session_id(None)
        );
        if recording.is_some() {
            let error = mcp_error(
                error_codes::ACTION_BACKEND_UNAVAILABLE,
                "action_diagnostic_queue_full_setup requires the real action emitter, not the recording backend",
            );
            self.audit_action_error_with_details_for_request(
                TOOL,
                &error,
                &request_details,
                &request_context,
            )?;
            return Err(error);
        }
        let from = audit_try!(
            self,
            TOOL,
            &request_context,
            synapse_action::backend::software::cursor_position()
                .map_err(diagnostic_action_error_to_mcp)
        );
        let response = audit_try!(
            self,
            TOOL,
            &request_context,
            queue_full_setup_with_lease(
                &mut _foreground_lease,
                &handle,
                from,
                params.blocker_duration_ms,
            )
            .await
        );
        self.audit_action_ok_with_details_for_request(
            TOOL,
            &json!({
                "response": response,
                "preflight": preflight,
            }),
            &request_context,
        )?;
        Ok(Json(response))
    }

    #[tool(description = "Release all held keyboard, mouse, and gamepad input state")]
    pub async fn release_all(
        &self,
        params: Parameters<ReleaseAllParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ReleaseAllResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "release_all",
            "tool.invocation kind=release_all"
        );
        let (handle, snapshot_handle, reflex_runtime) = self.m2_release_all_context()?;
        let result =
            release_all_with_handles(handle, snapshot_handle, reflex_runtime, params.0).await;
        self.audit_action_result_for_request_best_effort("release_all", &result, &request_context);
        result.map(Json)
    }
}

fn foreground_lease_session_id(
    request_context: &RequestContext<RoleServer>,
) -> Result<Option<String>, ErrorData> {
    super::context::mcp_session_id_from_request_context(request_context)
}

impl SynapseService {
    fn foreground_click_policy_for_request(
        &self,
        params: &ActClickParams,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<ForegroundClickPolicy, ErrorData> {
        let session_id = foreground_lease_session_id(request_context)?;
        // #187: the real-foreground SendInput tier is authorized here (profile,
        // or claim + session target) and re-proved against the live OS
        // foreground right before input is sent. A denial is carried, not
        // raised, so background tiers (UIA, PostMessage, CDP) still run.
        let gate = self.foreground_click_gate(session_id.as_deref(), params);
        // #258: the SendInput tier moves the human's real cursor; it is
        // re-checked against physical user activity right before delivery.
        let user_active = self.user_active_gate("act_click", session_id.as_deref())?;
        Ok(ForegroundClickPolicy::authorized(session_id, gate).with_user_active_gate(user_active))
    }

    fn foreground_click_gate(
        &self,
        session_id: Option<&str>,
        params: &ActClickParams,
    ) -> Result<crate::m2::ForegroundClickAuth, ErrorData> {
        // No session id means the single-client stdio admin, which the profile
        // snapshot also treats as break_glass.
        let profile_allows = self
            .tool_profile_snapshot(session_id)?
            .profile
            .allows_foreground_tier();
        let facts = match session_id {
            Some(session_id) if !profile_allows => self.click_claim_facts(session_id, params),
            _ => crate::m2::ClaimFacts {
                target_root: None,
                claim_held: false,
                session_target_matches: false,
            },
        };
        crate::m2::authorize_foreground_click(profile_allows, &facts)
    }

    /// Claim facts for a click's target window. Unreadable state is treated as
    /// "not claimed" so the gate fails closed.
    fn click_claim_facts(
        &self,
        session_id: &str,
        params: &ActClickParams,
    ) -> crate::m2::ClaimFacts {
        let target_root = click_target_root_hwnd(params).ok().flatten();
        let target = target_root.map(super::target_claims::window_session_target);
        let claim_held = target.as_ref().is_some_and(|target| {
            self.target_claim_for_session(session_id, target)
                .ok()
                .flatten()
                .is_some()
        });
        let session_target_matches = target.as_ref().is_some_and(|target| {
            self.session_target(Some(session_id))
                .ok()
                .flatten()
                .as_ref()
                == Some(target)
        });
        crate::m2::ClaimFacts {
            target_root,
            claim_held,
            session_target_matches,
        }
    }
}

fn audit_target_claim_denial<T: Serialize>(
    service: &SynapseService,
    tool: &'static str,
    error: ErrorData,
    request_context: &RequestContext<RoleServer>,
) -> Result<Json<T>, ErrorData> {
    let result: Result<T, ErrorData> = Err(error);
    service.audit_action_result_for_request(tool, &result, request_context)?;
    result.map(Json)
}

fn click_claim_target(params: &ActClickParams) -> Option<SessionTarget> {
    let root = click_target_root_hwnd(params).ok().flatten()?;
    Some(match &params.target {
        crate::m2::ActClickTarget::Element(element) => {
            web_element_claim_target(&element.element_id, root)
        }
        _ => super::target_claims::window_session_target(root),
    })
}

#[cfg(windows)]
fn preflight_background_webview_uia_click(params: &ActClickParams) -> Result<(), ErrorData> {
    preflight_background_webview_uia_click_with(
        params,
        click_target_root_hwnd,
        synapse_a11y::webview2_cdp_diagnostics_for_window,
        live_foreground_root_hwnd,
    )
}

/// Live foreground probe for [`preflight_background_webview_uia_click_with`]:
/// the top-level root of the current OS foreground window.
#[cfg(windows)]
fn live_foreground_root_hwnd() -> Result<i64, ErrorData> {
    let foreground = synapse_a11y::current_foreground_context().map_err(|error| {
        mcp_error(
            error.code(),
            format!("act_click WebView2 foreground preflight failed: {error}"),
        )
    })?;
    synapse_a11y::top_level_root_hwnd(foreground.hwnd).map_err(|error| {
        let error = synapse_a11y::as_foreground_error(error);
        mcp_error(
            error.code(),
            format!("act_click WebView2 foreground root readback failed: {error}"),
        )
    })
}

/// WebView2 background-click preflight with injected sources: `target_root`
/// resolves the click's top-level root (`None` = nothing to check),
/// `cdp_diagnostics` reads the WebView2 CDP diagnostics of that root (`None` =
/// not a WebView2 host), and `foreground_root` reads the foreground's root.
#[cfg(windows)]
fn preflight_background_webview_uia_click_with(
    params: &ActClickParams,
    target_root: impl FnOnce(&ActClickParams) -> Result<Option<i64>, ErrorData>,
    cdp_diagnostics: impl FnOnce(i64) -> Option<synapse_core::CdpDiagnostics>,
    foreground_root: impl FnOnce() -> Result<i64, ErrorData>,
) -> Result<(), ErrorData> {
    if let crate::m2::ActClickTarget::Element(element) = &params.target
        && synapse_a11y::cdp_backend_from_element_id(&element.element_id).is_some()
    {
        return Ok(());
    }
    // #79/#107: this preflight must cover both the UIA-element click surface and
    // the target-scoped raw point click surface (m2::click::point_post). Both can
    // route a claimed-but-background click through child-window HWND PostMessage,
    // and Chromium's WebView2 renderer does not consume synthetic WM_* mouse
    // messages without an attached CDP session, so either surface can otherwise
    // report a delivered click that produced no observed effect (#79).
    let Some(target_root) = target_root(params)? else {
        return Ok(());
    };
    let Some(cdp) = cdp_diagnostics(target_root) else {
        return Ok(());
    };
    let foreground_root = foreground_root()?;
    if foreground_root == target_root {
        return Ok(());
    }
    Err(webview2_background_click_refusal_error(
        click_target_kind_label(&params.target),
        &cdp,
        target_root,
        foreground_root,
    ))
}

#[cfg(not(windows))]
fn preflight_background_webview_uia_click(_params: &ActClickParams) -> Result<(), ErrorData> {
    Ok(())
}

/// #79/#107: distinguishes the UIA-element and raw-point click surfaces in the
/// refusal message below so the guidance names the surface that was refused.
#[cfg_attr(not(windows), allow(dead_code))]
fn click_target_kind_label(target: &crate::m2::ActClickTarget) -> &'static str {
    match target {
        crate::m2::ActClickTarget::Element(_) => "UIA element",
        crate::m2::ActClickTarget::Point(_) => "point",
    }
}

/// #79/#107: a claimed-but-background WebView2 target has no verified PostMessage
/// transport. Pure error construction (no Windows API calls) so routing/message
/// correctness is unit-testable without a live WebView2 window.
#[cfg_attr(not(windows), allow(dead_code))]
fn webview2_background_click_refusal_error(
    kind: &'static str,
    cdp: &synapse_core::CdpDiagnostics,
    target_root: i64,
    foreground_root: i64,
) -> ErrorData {
    let (code, route) = if cdp.status == synapse_core::CdpStatus::Ok {
        (
            error_codes::ACTION_TARGET_INVALID,
            "Use a CDP element id or a target-scoped CDP coordinate click for this WebView2 page",
        )
    } else {
        (
            error_codes::A11Y_CDP_UNREACHABLE,
            "Launch the app through Synapse process launch with cdp_debug=true, then use a CDP element or target-scoped coordinate click",
        )
    };
    mcp_error(
        code,
        format!(
            "act_click refused background WebView2 {kind} click before delivery: target HWND 0x{target_root:x}, foreground HWND 0x{foreground_root:x}. A synthetic PostMessage click is not consumed by the WebView2/Chromium renderer and can silently report delivered with no observed effect. {route}"
        ),
    )
}

fn element_claim_target(element_id: &ElementId) -> Option<SessionTarget> {
    element_id
        .parts()
        .ok()
        .map(|parts| web_element_claim_target(element_id, parts.hwnd))
}

/// #202: `act_press` keys go to the press dispatch target (an explicit
/// `cdp_target_id`, the session target, or the window's active tab), not to the
/// auto-wait element's tab, so the auto-wait element is checked at its whole
/// window: narrowing it to the element's tab would let keys reach a peer's tab.
/// The dispatch target itself is checked separately.
fn press_claim_target(params: &ActPressParams) -> Option<SessionTarget> {
    params
        .auto_wait_element_id
        .as_ref()
        .and_then(|element_id| element_id.parts().ok())
        .map(|parts| super::target_claims::window_session_target(parts.hwnd))
}

/// The claim targets `act_press` must clear: the dispatch target (an explicit
/// `window_hwnd`/`cdp_target_id`; `None` means the session target) and the
/// auto-wait element's whole window (#202).
fn press_claim_targets(
    params: &ActPressParams,
) -> Result<(Option<SessionTarget>, Option<SessionTarget>), ErrorData> {
    let dispatch =
        super::explicit_action_target(params.window_hwnd, params.cdp_target_id.as_deref())?;
    Ok((dispatch, press_claim_target(params)))
}

/// #202: a CDP web element is claimed at its tab, not its whole browser window,
/// so a session acting in its own tab is not blocked by another session's tab
/// in the same window (a window claim still overlaps every tab). Native and
/// legacy backend-only element ids keep the window target.
fn web_element_claim_target(element_id: &ElementId, hwnd: i64) -> SessionTarget {
    match synapse_a11y::cdp_target_from_element_id(element_id) {
        Some(cdp_target_id) if synapse_a11y::cdp_backend_from_element_id(element_id).is_some() => {
            SessionTarget::Cdp {
                window_hwnd: hwnd,
                cdp_target_id,
            }
        }
        _ => super::target_claims::window_session_target(hwnd),
    }
}

fn click_auto_wait_element_id(params: &ActClickParams) -> Option<&ElementId> {
    match &params.target {
        crate::m2::ActClickTarget::Element(element) => Some(&element.element_id),
        crate::m2::ActClickTarget::Point(_) => None,
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ActionabilityAutoWaitRequirement {
    Action,
    Editable,
}

impl ActionabilityAutoWaitRequirement {
    const fn label(self) -> &'static str {
        match self {
            Self::Action => "action_ready",
            Self::Editable => "editable_action_ready",
        }
    }

    const fn includes_editable(self) -> bool {
        matches!(self, Self::Editable)
    }
}

async fn maybe_auto_wait_for_actionability(
    service: &SynapseService,
    tool: &'static str,
    request_context: &RequestContext<RoleServer>,
    enabled: bool,
    element_id: Option<&ElementId>,
    timeout_ms: u32,
    explicit_window_hwnd: Option<i64>,
    explicit_cdp_target_id: Option<&str>,
    requirement: ActionabilityAutoWaitRequirement,
) -> Result<(), ErrorData> {
    if !enabled {
        return Ok(());
    }
    crate::m2::validate_auto_wait_timeout(tool, enabled, timeout_ms)?;
    let element_id = element_id.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} auto_wait=true requires a CDP web element_id target"),
        )
    })?;
    #[cfg(windows)]
    {
        auto_wait_for_actionability(
            service,
            tool,
            request_context,
            element_id,
            timeout_ms,
            explicit_window_hwnd,
            explicit_cdp_target_id,
            requirement,
        )
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = (
            service,
            request_context,
            element_id,
            explicit_window_hwnd,
            explicit_cdp_target_id,
            requirement,
        );
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            format!("{tool} auto_wait actionability is only available on Windows in this build"),
        ))
    }
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
async fn auto_wait_for_actionability(
    service: &SynapseService,
    tool: &'static str,
    request_context: &RequestContext<RoleServer>,
    element_id: &ElementId,
    timeout_ms: u32,
    explicit_window_hwnd: Option<i64>,
    explicit_cdp_target_id: Option<&str>,
    requirement: ActionabilityAutoWaitRequirement,
) -> Result<(), ErrorData> {
    let backend_node_id =
        synapse_a11y::cdp_backend_from_element_id(element_id).ok_or_else(|| {
            mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!("{tool} auto_wait requires a CDP web element_id, got {element_id}"),
            )
        })?;
    let element_window_hwnd = element_id
        .parts()
        .map_err(|error| {
            mcp_error(
                error_codes::ACTION_ELEMENT_NOT_RESOLVED,
                format!("{tool} auto_wait element id is malformed: {error}"),
            )
        })?
        .hwnd;
    if let Some(window_hwnd) = explicit_window_hwnd
        && window_hwnd != element_window_hwnd
    {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "{tool} auto_wait element window {element_window_hwnd:#x} does not match explicit action window {window_hwnd:#x}"
            ),
        ));
    }
    if explicit_cdp_target_id.is_some() && explicit_window_hwnd.is_none() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} auto_wait with explicit cdp_target_id requires window_hwnd"),
        ));
    }
    let element_target_id = synapse_a11y::cdp_target_from_element_id(element_id);
    if let (Some(explicit), Some(encoded)) = (explicit_cdp_target_id, element_target_id.as_deref())
        && !explicit.eq_ignore_ascii_case(encoded)
    {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "{tool} auto_wait element target {encoded:?} does not match explicit action target {explicit:?}"
            ),
        ));
    }
    let Some(session_id) = super::context::mcp_session_id_from_request_context(request_context)?
    else {
        return Err(mcp_error(
            error_codes::TARGET_NOT_SET,
            format!(
                "{tool} auto_wait requires an MCP session target; refusing to use the human foreground tab"
            ),
        ));
    };
    let target_id_param = explicit_cdp_target_id.or(element_target_id.as_deref());
    let (window_hwnd, cdp_target_id) = service.resolve_cdp_tab_mutation_target(
        tool,
        &session_id,
        explicit_window_hwnd.or(Some(element_window_hwnd)),
        target_id_param,
    )?;
    let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
        return Err(mcp_error(
            error_codes::A11Y_CDP_UNREACHABLE,
            format!(
                "{tool} auto_wait requires a reachable raw CDP endpoint for window_hwnd {window_hwnd:#x}"
            ),
        ));
    };
    let scroll =
        synapse_a11y::cdp_scroll_into_view_node(&endpoint, &cdp_target_id, backend_node_id)
            .await
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("{tool} auto_wait scrollIntoViewIfNeeded failed: {error}"),
                )
            })?;
    let started = Instant::now();
    let deadline = started
        .checked_add(Duration::from_millis(u64::from(timeout_ms)))
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{tool} auto_wait_timeout_ms {timeout_ms} overflowed this host clock"),
            )
        })?;
    let mut poll_count = 0_u32;
    loop {
        let actionability =
            synapse_a11y::cdp_actionability(&endpoint, &cdp_target_id, backend_node_id)
                .await
                .map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!("{tool} auto_wait actionability readback failed: {error}"),
                    )
                })?;
        poll_count = poll_count.saturating_add(1);
        if actionability_satisfies_requirement(&actionability, requirement) {
            tracing::info!(
                code = "M2_ACTIONABILITY_AUTO_WAIT_READY",
                tool,
                element_id = %element_id,
                hwnd = window_hwnd,
                cdp_target_id = %actionability.target_id,
                backend_node_id,
                requirement = requirement.label(),
                poll_count,
                elapsed_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
                window_scroll_changed = scroll.window_scroll_changed,
                container_scroll_changed = scroll.container_scroll_changed,
                "readback=DOM.scrollIntoViewIfNeeded+CDP.actionability outcome=auto_wait_ready"
            );
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(actionability_auto_wait_timeout_error(
                tool,
                element_id,
                backend_node_id,
                &cdp_target_id,
                timeout_ms,
                requirement,
                poll_count,
                Some(&actionability),
                &scroll,
            ));
        }
        let delay = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(50));
        tokio::time::sleep(delay).await;
    }
}

#[cfg(windows)]
fn actionability_satisfies_requirement(
    actionability: &synapse_a11y::CdpActionabilityResult,
    requirement: ActionabilityAutoWaitRequirement,
) -> bool {
    match requirement {
        ActionabilityAutoWaitRequirement::Action => actionability.action_ready,
        ActionabilityAutoWaitRequirement::Editable => actionability.editable_action_ready,
    }
}

fn actionability_failure_is_relevant(
    predicate: &str,
    requirement: ActionabilityAutoWaitRequirement,
) -> bool {
    matches!(
        predicate,
        "attached" | "visible" | "stable" | "enabled" | "receives_events"
    ) || (requirement.includes_editable() && predicate == "editable")
}

#[cfg(windows)]
fn actionability_relevant_failures(
    actionability: &synapse_a11y::CdpActionabilityResult,
    requirement: ActionabilityAutoWaitRequirement,
) -> Vec<Value> {
    actionability
        .failure_reasons
        .iter()
        .filter(|failure| actionability_failure_is_relevant(&failure.predicate, requirement))
        .map(|failure| {
            json!({
                "predicate": failure.predicate,
                "reason": failure.reason,
                "detail": failure.detail,
            })
        })
        .collect()
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn actionability_auto_wait_timeout_error(
    tool: &'static str,
    element_id: &ElementId,
    backend_node_id: i64,
    cdp_target_id: &str,
    timeout_ms: u32,
    requirement: ActionabilityAutoWaitRequirement,
    poll_count: u32,
    last_actionability: Option<&synapse_a11y::CdpActionabilityResult>,
    scroll: &synapse_a11y::CdpScrollIntoViewResult,
) -> ErrorData {
    let unmet_predicates = last_actionability
        .map(|actionability| actionability_relevant_failures(actionability, requirement))
        .unwrap_or_default();
    let predicate_labels = unmet_predicates
        .iter()
        .filter_map(|failure| failure.get("predicate").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let predicate_detail = if predicate_labels.is_empty() {
        "unknown".to_owned()
    } else {
        predicate_labels.join(",")
    };
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} auto_wait timed out after {timeout_ms} ms waiting for {}; unmet predicates: {predicate_detail}",
            requirement.label()
        ),
        Some(json!({
            "code": error_codes::BROWSER_WAIT_TIMEOUT,
            "tool": tool,
            "element_id": element_id.to_string(),
            "backend_node_id": backend_node_id,
            "cdp_target_id": cdp_target_id,
            "timeout_ms": timeout_ms,
            "requirement": requirement.label(),
            "poll_count": poll_count,
            "unmet_predicates": unmet_predicates,
            "last_actionability": last_actionability.and_then(|value| serde_json::to_value(value).ok()),
            "scroll": serde_json::to_value(scroll).ok(),
            "source_of_truth": "DOM.scrollIntoViewIfNeeded + DOM.getBoxModel + DOM.getNodeForLocation + elementFromPoint",
        })),
    )
}

/// The claim targets an `act_stroke` touches (#202). A stroke routed into the
/// session's CDP tab touches only that tab (its path points are page
/// coordinates, not screen pixels). Otherwise: the CDP aim element's tab, or the
/// top-level window under every point of the planned path (deduped).
fn stroke_claim_targets(
    route: &ActStrokeRoute,
    plan: &crate::m2::ActStrokePlan,
    root_at: impl Fn(synapse_core::Point) -> Option<i64>,
) -> Vec<SessionTarget> {
    if let ActStrokeRoute::CdpTarget {
        window_hwnd,
        cdp_target_id,
    } = route
    {
        return vec![SessionTarget::Cdp {
            window_hwnd: *window_hwnd,
            cdp_target_id: cdp_target_id.clone(),
        }];
    }
    if let Some(element_id) = plan.cdp_aim_element() {
        return element_claim_target(element_id).into_iter().collect();
    }
    let mut roots: Vec<i64> = plan
        .screen_points()
        .into_iter()
        .filter_map(root_at)
        .collect();
    roots.sort_unstable();
    roots.dedup();
    roots
        .into_iter()
        .map(super::target_claims::window_session_target)
        .collect()
}

/// How `act_stroke` dispatches (#202). Decided once per call; the same value
/// drives the claim check and the dispatch so the two cannot diverge.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ActStrokeRoute {
    /// Background `Input.dispatchMouseEvent` into the session's CDP tab; the
    /// path points are page coordinates of that tab.
    CdpTarget {
        window_hwnd: i64,
        cdp_target_id: String,
    },
    /// The session target is a CDP tab but the plan has no explicit path:
    /// refused rather than falling back to the real cursor.
    CdpTargetRefused,
    /// The action-handle route: screen input, or a CDP element aim.
    Screen,
}

/// Decides the `act_stroke` route. `screen_input_forced` is true when a
/// recording is active or the hardware backend was requested.
fn act_stroke_route(
    session_target: Option<SessionTarget>,
    plan: &crate::m2::ActStrokePlan,
    screen_input_forced: bool,
) -> ActStrokeRoute {
    if screen_input_forced || plan.is_cdp_element_aim() {
        return ActStrokeRoute::Screen;
    }
    let Some(SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id,
    }) = session_target
    else {
        return ActStrokeRoute::Screen;
    };
    if !plan.can_try_cdp_target_stroke() {
        return ActStrokeRoute::CdpTargetRefused;
    }
    ActStrokeRoute::CdpTarget {
        window_hwnd,
        cdp_target_id,
    }
}

/// #191: point-scoped pixel evidence is captured from the window under the
/// point (where `at=` input is delivered), not from the foreground window, so a
/// visible but unfocused target is still observable. The window is resolved
/// once per action and carried to both captures, so the before and after
/// evidence always come from the same window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DeltaRegion {
    point: Option<Point>,
    target_window: Option<i64>,
}

impl DeltaRegion {
    /// `root_at` maps the point to its top-level window; it is never called
    /// without a point, so no point never invents a target window.
    fn resolve(point: Option<Point>, root_at: impl Fn(Point) -> Option<i64>) -> Self {
        Self {
            point,
            target_window: point_region_target_window_with(point, root_at),
        }
    }
}

fn point_region_target_window_with(
    point_region: Option<Point>,
    root_at: impl Fn(Point) -> Option<i64>,
) -> Option<i64> {
    point_region.and_then(root_at)
}

fn scroll_claim_target(params: &ActScrollParams) -> Option<SessionTarget> {
    if let Some(target) = &params.target {
        return element_claim_target(&target.element_id);
    }
    // Use the point's root for the claim check and pin dispatch to that root
    // (#62); `ensure_explicit_input_target_claim` requires this session's
    // claim on it, so a point over an unclaimed window is refused.
    point_region_target_window_with(params.at.map(Into::into), crate::m2::root_hwnd_at_point)
        .map(super::target_claims::window_session_target)
}

const fn click_delta_source_of_truth(target_window_hwnd: Option<i64>) -> &'static str {
    if target_window_hwnd.is_some() {
        "target_window_ui_or_pixels"
    } else {
        "foreground_focused_ui_or_pixels"
    }
}

fn visual_delta_text_integrity(source_of_truth: &str) -> String {
    format!("verify_delta_visual_readback:{source_of_truth}")
}

impl SynapseService {
    /// Arbitrary native input targets require this session's exact live claim (#62).
    fn ensure_explicit_input_target_claim(
        &self,
        tool: &'static str,
        target: Option<&SessionTarget>,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let Some(session_id) =
            super::context::mcp_session_id_from_request_context(request_context)?
        else {
            return Ok(());
        };
        if let Some(target) = target {
            self.ensure_target_claim_allows_session(tool, &session_id, target)?;
            if self
                .target_claim_for_session(&session_id, target)?
                .is_some()
            {
                return Ok(());
            }
        }
        Err(mcp_error(
            error_codes::TARGET_CLAIM_NOT_FOUND,
            format!(
                "{tool} refused before delivery: explicit input target is not claimed by this MCP session; no input was sent"
            ),
        ))
    }

    /// #6: true when this MCP session owns a live `target_claim` on the exact
    /// window `act_focus_window` resolves. That claim (plus the foreground lease
    /// acquired later in the tool and the profile scope checked by the action
    /// gate) is the proof that lets focus recovery run with no OS foreground.
    fn focus_recovery_claim_proven(
        &self,
        params: &ActFocusWindowParams,
        request_context: &RequestContext<RoleServer>,
    ) -> bool {
        let session_id = super::context::mcp_session_id_from_request_context(request_context)
            .ok()
            .flatten();
        self.focus_recovery_claim_proven_for(
            act_focus_window_target_hwnd(params).ok(),
            session_id.as_deref(),
        )
    }

    pub(crate) fn focus_recovery_claim_proven_for(
        &self,
        target_hwnd: Option<i64>,
        session_id: Option<&str>,
    ) -> bool {
        let (Some(hwnd), Some(session_id)) = (target_hwnd, session_id) else {
            return false;
        };
        let target = super::target_claims::window_session_target(hwnd);
        matches!(
            self.target_claim_for_session(session_id, &target),
            Ok(Some(_))
        )
    }

    /// #7: true when this point click is a session-claimed background route that
    /// needs no OS foreground. Mirrors the later `current_point_click_route`
    /// inputs (root resolved per `point_click_resolves_root`), so the action gate
    /// skips the live-foreground requirement exactly when delivery will not use it.
    fn claimed_background_point_click(
        &self,
        params: &ActClickParams,
        request_context: &RequestContext<RoleServer>,
    ) -> bool {
        if !point_click_resolves_root(params) {
            return false;
        }
        let Ok(root) = click_target_root_hwnd(params) else {
            return false;
        };
        let Ok(claim) = self.point_click_session_claim(params, root, request_context) else {
            return false;
        };
        claimed_background_point_possible(params, root, claim)
    }

    /// #107: this session's live target_claim on a point click's target root, read
    /// from the claim registry. `None` when the request has no session, no target
    /// window, or the session does not own a claim on that window.
    fn point_click_session_claim(
        &self,
        params: &ActClickParams,
        target_root: Option<i64>,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<Option<ClaimedPointTarget>, ErrorData> {
        let (crate::m2::ActClickTarget::Point(_), Some(root)) = (&params.target, target_root)
        else {
            return Ok(None);
        };
        let Some(session_id) =
            super::context::mcp_session_id_from_request_context(request_context)?
        else {
            return Ok(None);
        };
        let target = super::target_claims::window_session_target(root);
        Ok(self
            .target_claim_for_session(&session_id, &target)?
            .map(|claim| ClaimedPointTarget::from_session_claim(root, claim.generation)))
    }

    /// The re-check typing and keys share right before the input, after the
    /// lease: target, claim, foreground AND owner idle (#2340 part B, B4).
    fn ensure_claimed_input_target(
        &self,
        tool: &'static str,
        window_hwnd: Option<i64>,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?;
        self.ensure_claimed_input_target_for_session(tool, window_hwnd, session_id.as_deref())?;
        self.ensure_user_idle(tool, session_id.as_deref())
    }

    /// #108 / #2340 part B: real typing or key input without break_glass is
    /// allowed only into the session's own claimed window while it is in front.
    pub(crate) fn ensure_claimed_input_target_for_session(
        &self,
        tool: &'static str,
        window_hwnd: Option<i64>,
        session_id: Option<&str>,
    ) -> Result<(), ErrorData> {
        let input = match tool {
            "act_press" => "key press",
            "act_stroke" => "drag input",
            _ => "typing",
        };
        let session_id = session_id.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PROFILE_POLICY_DENIED,
                format!("claimed {input} requires an MCP session"),
            )
        })?;
        let hwnd = window_hwnd.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PROFILE_POLICY_DENIED,
                format!("claimed {input} requires a target window readback"),
            )
        })?;
        if !self.claimed_window_in_front(session_id, hwnd)? {
            return Err(mcp_error(
                error_codes::TOOL_PROFILE_POLICY_DENIED,
                format!("claimed {input} requires the session's claimed window to be foreground"),
            ));
        }
        Ok(())
    }

    /// The session's target is this window, the session holds its claim, and
    /// it is the OS foreground window.
    fn claimed_window_in_front(&self, session_id: &str, hwnd: i64) -> Result<bool, ErrorData> {
        let target = SessionTarget::Window { hwnd };
        Ok(
            self.session_target(Some(session_id))? == Some(target.clone())
                && self
                    .target_claim_for_session(session_id, &target)?
                    .is_some()
                && self.current_audit_foreground()?.hwnd == hwnd,
        )
    }

    fn acquire_claimed_input_lease(
        &self,
        tool: &'static str,
        window_hwnd: Option<i64>,
        ttl_ms: u64,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<crate::m2::ForegroundInputLeaseGuard, ErrorData> {
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?;
        self.ensure_claimed_input_target_for_session(tool, window_hwnd, session_id.as_deref())?;
        let Some(session_id) = session_id else {
            // The check above already refused a request without a session.
            return Err(mcp_error(
                error_codes::TOOL_PROFILE_POLICY_DENIED,
                format!("{tool} claimed input requires an MCP session"),
            ));
        };
        // #258: claimed input still sends real keystrokes.
        self.ensure_user_idle(tool, Some(&session_id))?;
        crate::m2::acquire_foreground_input_lease_with_ttl(tool, Some(&session_id), ttl_ms)
    }
}

fn acquire_tool_foreground_input_lease(
    service: &SynapseService,
    tool: &'static str,
    request_context: &RequestContext<RoleServer>,
) -> Result<crate::m2::ForegroundInputLeaseGuard, ErrorData> {
    acquire_tool_foreground_input_lease_with_ttl(
        service,
        tool,
        request_context,
        synapse_action::DEFAULT_LEASE_TTL_MS,
    )
}

fn acquire_tool_foreground_input_lease_with_ttl(
    service: &SynapseService,
    tool: &'static str,
    request_context: &RequestContext<RoleServer>,
    ttl_ms: u64,
) -> Result<crate::m2::ForegroundInputLeaseGuard, ErrorData> {
    // `None` here means the stdio transport by construction — an HTTP request
    // without Mcp-Session-Id fails hard upstream. stdio is single-client, so
    // the stable "stdio" owner (the same idiom the m3 layer uses) gives the
    // lease registry a real owner instead of refusing every foreground tier
    // over stdio.
    let caller_session_id = foreground_lease_session_id(request_context)?;
    if let Some(caller_session_id) = caller_session_id.as_deref() {
        let profile = service
            .tool_profile_snapshot(Some(caller_session_id))?
            .profile;
        if !profile.allows_foreground_tier() {
            return Err(mcp_error(
                error_codes::TOOL_PROFILE_POLICY_DENIED,
                format!("{tool} requires break_glass or full_capability for OS foreground input"),
            ));
        }
    }
    // #258: every foreground tier here (focus_window, SendInput type/press/
    // scroll/stroke/keymap, cursor blocker) moves the human's real focus,
    // cursor or keyboard; refuse before the lease while the human is active.
    service.ensure_user_idle(tool, caller_session_id.as_deref())?;
    let session_id = caller_session_id.unwrap_or_else(|| "stdio".to_owned());
    crate::m2::acquire_foreground_input_lease_with_ttl(tool, Some(&session_id), ttl_ms)
}

fn lease_ttl_for_hold_ms(hold_ms: u32) -> u64 {
    crate::m2::foreground_input_lease_ttl_for_hold_ms(hold_ms)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClickDeltaSignature {
    foreground_hwnd: i64,
    foreground_pid: u32,
    foreground_process: String,
    foreground_title: String,
    foreground_title_sha256: Option<String>,
    focused_element_id: Option<String>,
    focused_role: Option<String>,
    focused_name_sha256: Option<String>,
    focused_value_sha256: Option<String>,
    focused_bbox: Option<Rect>,
    element_count: usize,
    elements_sha256: String,
    /// #107: element fingerprint WITHOUT the per-element `focused` flag, so a
    /// click's effect can be told apart from focus merely moving.
    effect_elements_sha256: String,
    cdp_status: Option<String>,
    cdp_endpoint_present: bool,
    web_path: Option<String>,
    cursor_position: Option<Point>,
    pixel: ClickPixelSignature,
    point_pixel: Option<ClickPixelSignature>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CdpKeyboardDeltaSignature {
    target_id: String,
    has_active_element: bool,
    tag_name: String,
    id_sha256: Option<String>,
    name_sha256: Option<String>,
    value_len: usize,
    value_sha256: String,
    selection_start: Option<u32>,
    selection_end: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct HwndKeyboardDeltaSignature {
    target: HwndKeyboardTargetState,
    /// Windows clipboard sequence number at capture time. Lets the PostMessage
    /// keyboard verify observe clipboard-mutating chords (Ctrl+C/Ctrl+X) whose
    /// effect is NOT a target text/selection change (#1331).
    clipboard_sequence: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HwndKeyboardExpectedEffect {
    AnyDelta,
    PrintableText {
        text: String,
    },
    SelectAll,
    /// Ctrl+C / Ctrl+X: success is observed as a clipboard sequence-number change.
    Clipboard,
}

// Native foreground readback is an opportunistic observation, not a wait for
// the chord to create a window. Explicit delta verification keeps its settle time.
fn action_delta_verification_delay(source_of_truth: &str, timeout_ms: u32) -> Duration {
    if source_of_truth == "foreground_related_window" {
        Duration::ZERO
    } else {
        Duration::from_millis(u64::from(timeout_ms))
    }
}

#[derive(Clone, Debug)]
struct RelatedWindowEvidence {
    prior_windows: Vec<i64>,
    owner_chain: Vec<(i64, u32)>,
    /// #92: PIDs in the target's process tree (children, grandchildren, ...),
    /// filled at verification time only when the foreground window is new.
    descendant_pids: Vec<u32>,
}

#[cfg(windows)]
fn related_window_inventory() -> Result<Vec<i64>, ErrorData> {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible};
    use windows::core::BOOL;

    /// `EnumWindows` callback that collects the visible top-level window handles.
    ///
    /// # Safety
    ///
    /// `data` must be the `LPARAM` built from a live, exclusively borrowed
    /// `Vec<i64>` (as `related_window_inventory` passes to `EnumWindows`) and
    /// stay valid for the whole synchronous enumeration; `hwnd` must be a window
    /// handle supplied by `EnumWindows`. Only `EnumWindows` should call this.
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: the function contract above - `data` is the live, exclusive Vec
        // pointer `EnumWindows` hands back while `related_window_inventory` waits.
        let windows = unsafe { &mut *(data.0 as *mut Vec<i64>) };
        // SAFETY: IsWindowVisible only reads the style of the enumerated handle.
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            windows.push(hwnd.0 as i64);
        }
        BOOL(1)
    }
    let mut windows = Vec::<i64>::new();
    // SAFETY: the callback does not retain the pointer beyond this synchronous call.
    unsafe { EnumWindows(Some(collect), LPARAM((&raw mut windows) as isize)) }.map_err(
        |error| {
            mcp_error(
                error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE,
                error.to_string(),
            )
        },
    )?;
    Ok(windows)
}

#[cfg(not(windows))]
fn related_window_inventory() -> Result<Vec<i64>, ErrorData> {
    Err(mcp_error(
        error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE,
        "related-window inventory requires Windows",
    ))
}

/// The owner chain of `hwnd` (owner hwnd + pid, nearest owner first); the
/// bounded, cycle-safe Win32 walk lives in `synapse_a11y` (empty off Windows).
fn related_window_owner_chain(hwnd: i64) -> Vec<(i64, u32)> {
    synapse_a11y::window_owner_chain(hwnd)
}

#[derive(Clone, Debug)]
struct ForegroundChangePolicy {
    allow: bool,
    related_window: Option<RelatedWindowEvidence>,
    expected_process_regex: Option<regex::Regex>,
    expected_process_pattern: Option<String>,
    expected_title_regex: Option<regex::Regex>,
    expected_title_pattern: Option<String>,
}

impl ForegroundChangePolicy {
    fn reject() -> Self {
        Self {
            allow: false,
            related_window: None,
            expected_process_regex: None,
            expected_process_pattern: None,
            expected_title_regex: None,
            expected_title_pattern: None,
        }
    }

    fn has_expectations(&self) -> bool {
        self.expected_process_regex.is_some() || self.expected_title_regex.is_some()
    }
}

#[derive(Clone, Debug)]
struct ActTypeTextReadback {
    signature: ActTypeTextSignature,
    value: Option<String>,
    browser_url: Option<String>,
    /// Parent date control of a focused date segment (UIA-only `<input type=date>`, #80).
    date_parent: Option<act_type_date_parent::ActTypeDateParentReadback>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ActTypeForegroundFallbackTarget {
    element_id: String,
    root_hwnd: i64,
    process_name: String,
    role: String,
    automation_id_present: bool,
    bbox: Rect,
    enabled: bool,
    keyboard_focusable: bool,
    patterns: Vec<UiaPattern>,
    name_len: usize,
    value_len: Option<usize>,
}

#[derive(Clone, Debug)]
struct ActTypeBrowserUrlPolicy {
    expected_url_regex: regex::Regex,
    expected_url_pattern: String,
}

#[derive(Clone, Debug)]
struct CdpTargetUrlReadback {
    url: Option<String>,
    target_id: Option<String>,
    source: Option<String>,
}

#[derive(Clone, Debug)]
struct BrowserTargetReadback {
    url: CdpTargetUrlReadback,
    title: Option<String>,
    ready_state: Option<String>,
    active: Option<bool>,
    active_text: Option<CdpActiveTextReadback>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ActTypeTextSignature {
    foreground_hwnd: i64,
    foreground_pid: u32,
    foreground_process: String,
    foreground_title_sha256: Option<String>,
    focused_element_id: Option<String>,
    focused_role: Option<String>,
    focused_name_sha256: Option<String>,
    focused_value_len: Option<usize>,
    focused_value_sha256: Option<String>,
    focused_selected_text_sha256: Option<String>,
    focused_bbox: Option<Rect>,
    readback_source: Option<String>,
    has_text_readback: bool,
    text_readback_attempts: Vec<String>,
    cdp_status: Option<String>,
    cdp_endpoint_present: bool,
    cdp_selected_target_id: Option<String>,
    cdp_active_has_element: Option<bool>,
    cdp_active_is_editable: Option<bool>,
    cdp_active_tag_name: Option<String>,
    cdp_active_input_type: Option<String>,
    cdp_active_element_path: Option<String>,
    cdp_active_id_sha256: Option<String>,
    cdp_active_name_sha256: Option<String>,
    cdp_active_value_len: Option<usize>,
    cdp_active_value_sha256: Option<String>,
    cdp_active_error_code: Option<String>,
    cdp_active_error_detail_sha256: Option<String>,
    ocr_word_count: usize,
    ocr_text_len: Option<usize>,
    ocr_text_sha256: Option<String>,
    web_path: Option<String>,
    browser_url_len: Option<usize>,
    browser_url_sha256: Option<String>,
    browser_cdp_target_id: Option<String>,
    browser_url_readback_source: Option<String>,
    browser_title_sha256: Option<String>,
    browser_ready_state: Option<String>,
    browser_tab_active: Option<bool>,
}

#[derive(Clone, Debug)]
struct CdpActiveTextReadback {
    value: Option<String>,
    target_id: Option<String>,
    has_active_element: Option<bool>,
    is_editable: Option<bool>,
    tag_name: Option<String>,
    input_type: Option<String>,
    element_path: Option<String>,
    id_sha256: Option<String>,
    name_sha256: Option<String>,
    value_len: Option<usize>,
    value_sha256: Option<String>,
    error_code: Option<String>,
    error_detail_sha256: Option<String>,
    attempt: String,
}

#[derive(Clone, Debug)]
struct OcrTextReadback {
    value: Option<String>,
    word_count: usize,
    value_len: Option<usize>,
    value_sha256: Option<String>,
    attempt: String,
}

#[derive(Clone, Debug)]
struct ActTypeFocusedTextCandidate {
    element_id: String,
    role: String,
    name: String,
    selected_text: Option<String>,
    bbox: Rect,
    value: Option<String>,
    patterns: Vec<UiaPattern>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClickPixelSignature {
    status: String,
    region: Rect,
    bitmap_sha256: Option<String>,
    detail: Option<String>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ClickElementFingerprint {
    element_id: String,
    role: String,
    automation_id: Option<String>,
    name_sha256: Option<String>,
    value_sha256: Option<String>,
    bbox: Rect,
    enabled: bool,
    focused: bool,
    /// #189: checked/selected/expanded state, so a toggle that also moved
    /// focus still shows as an element change.
    control_state: Option<String>,
}

impl SynapseService {
    pub(crate) fn act_clipboard_for_session(
        &self,
        params: ActClipboardParams,
        session_id: &str,
        source_of_truth: &'static str,
    ) -> Result<ActClipboardResponse, ErrorData> {
        let result =
            act_clipboard_session_buffer(params, session_id, self.session_clipboards_ref());
        if let Err(error) = &result {
            tracing::error!(
                code = error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("code"))
                    .and_then(|value| value.as_str())
                    .unwrap_or(error_codes::TOOL_INTERNAL_ERROR),
                session_id,
                source_of_truth,
                detail = %error.message,
                data = ?error.data,
                "act_clipboard session buffer operation failed"
            );
        }
        result
    }

    async fn act_click_with_verified_router(
        &self,
        handle: ActionHandle,
        recording: Option<Arc<RecordingBackend>>,
        params: ActClickParams,
        before: ClickDeltaSignature,
        verify_timeout_ms: u32,
        target_window_hwnd: Option<i64>,
        foreground_click_policy: ForegroundClickPolicy,
        started: Instant,
    ) -> Result<ActClickResponse, ErrorData> {
        let port = LiveClickTierPort {
            handle,
            recording,
            policy: foreground_click_policy,
            service: self,
            before,
            verify_timeout_ms,
            target_window_hwnd,
        };
        route_verified_click(&port, params, started).await
    }

    fn reconcile_click_postdispatch_error(
        params: &ActClickParams,
        error: &ErrorData,
        postcondition: ActClickPostcondition,
        started: Instant,
    ) -> Result<ActClickResponse, ErrorData> {
        let mut tier_attempts = click_tier_attempts_from_error(error);
        let reconciled = reconciled_click_attempt(&tier_attempts, error);
        let tier = reconciled.tier.clone();
        tier_attempts.push(reconciled);
        let timing = synapse_action::cached_double_click_timing();
        tracing::info!(
            code = "M2_ACT_CLICK_POSTDISPATCH_READBACK_RECONCILED",
            kind = "act_click",
            backend_tier_used = %tier,
            error_message = %error.message,
            postcondition_status = %postcondition.status,
            "readback=target_window_delta tool=act_click outcome=reconciled_postdispatch_error"
        );
        Ok(ActClickResponse {
            ok: true,
            used_invoke_pattern: tier == "uia",
            backend_used: tier.clone(),
            backend_tier_used: tier,
            required_foreground: tier_attempts
                .iter()
                .any(|attempt| attempt.required_foreground),
            tier_attempts,
            postcondition,
            press_hold_ms: params.hold_ms,
            double_click_window_ms: timing.window_ms,
            inter_click_delay_ms: timing.inter_click_delay_ms,
            elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
        })
    }

    async fn capture_click_delta_signature(
        &self,
        max_elements: usize,
        target_window_hwnd: Option<i64>,
    ) -> Result<ClickDeltaSignature, ErrorData> {
        self.capture_action_delta_signature(max_elements, None, false, target_window_hwnd)
            .await
    }

    fn capture_coordinate_click_delta_signature(
        &self,
        hwnd: i64,
    ) -> Result<ClickDeltaSignature, ErrorData> {
        let window = synapse_a11y::foreground_context(hwnd)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        // #191: pixels come from the target window's own capture, not a screen region.
        let pixel_evidence =
            pixel_signature::capture_window_pixel_evidence(window.hwnd, window.window_bounds, None);
        Ok(ClickDeltaSignature {
            foreground_hwnd: window.hwnd,
            foreground_pid: window.pid,
            foreground_process: window.process_name,
            foreground_title_sha256: non_empty_sha256(&window.window_title),
            foreground_title: window.window_title,
            focused_element_id: None,
            focused_role: None,
            focused_name_sha256: None,
            focused_value_sha256: None,
            focused_bbox: None,
            element_count: 0,
            elements_sha256: hash_json(&Vec::<u8>::new())?,
            effect_elements_sha256: hash_json(&Vec::<u8>::new())?,
            cdp_status: None,
            cdp_endpoint_present: false,
            web_path: Some("pixel_only_coordinate".to_owned()),
            cursor_position: None,
            pixel: pixel_evidence.window,
            point_pixel: None,
        })
    }

    fn capture_native_foreground_signature(&self) -> Result<ClickDeltaSignature, ErrorData> {
        let foreground = synapse_a11y::current_foreground_context()
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        self.capture_coordinate_click_delta_signature(foreground.hwnd)
    }

    fn capture_raw_input_delta_signature(
        &self,
        target_window: Option<i64>,
        point: Option<Point>,
        include_cursor: bool,
    ) -> Result<ClickDeltaSignature, ErrorData> {
        let mut signature = match target_window {
            Some(hwnd) => self.capture_coordinate_click_delta_signature(hwnd)?,
            None => self.capture_native_foreground_signature()?,
        };
        if let Some(point) = point {
            let window = synapse_a11y::foreground_context(signature.foreground_hwnd)
                .map_err(|error| mcp_error(error.code(), error.to_string()))?;
            let evidence = pixel_signature::capture_window_pixel_evidence(
                window.hwnd,
                window.window_bounds,
                Some(point),
            );
            signature.pixel = evidence.window;
            signature.point_pixel = evidence.point;
        }
        if include_cursor {
            signature.cursor_position = synapse_action::backend::software::cursor_position().ok();
        }
        Ok(signature)
    }

    async fn capture_act_type_text_signature(
        &self,
        max_elements: usize,
        require_focused_text_value: bool,
        require_browser_url: bool,
        session_id: Option<&str>,
    ) -> Result<ActTypeTextReadback, ErrorData> {
        let mut input = {
            let state = self.m1_state()?;
            crate::m1::current_input(&state, 6)?
        };
        crate::m1::enrich_input_with_cdp(&mut input, 6, max_elements).await;
        crate::m1::enrich_input_with_browser_ocr(&mut input, max_elements);
        let bridge_target = self
            .chrome_bridge_session_target_readback(
                session_id,
                require_browser_url || require_focused_text_value,
            )
            .await?;
        let browser_url = match bridge_target.as_ref() {
            Some(readback) => readback.url.clone(),
            None => Self::cdp_selected_target_url(&input, require_browser_url).await?,
        };

        let focused = focused_text_candidate(input.focused.as_ref(), &input.elements);
        let (uia_value, uia_readback_source) = focused_text_readback(focused.as_ref());
        let cdp_readback = match bridge_target
            .as_ref()
            .and_then(|readback| readback.active_text.clone())
        {
            Some(readback) => readback,
            None => cdp_active_text_readback(&input).await,
        };
        let ocr_readback = ocr_focused_rect_text_readback(focused.as_ref(), &input.elements);
        let mut text_readback_attempts = Vec::new();
        text_readback_attempts.push(match uia_readback_source {
            Some(source) => format!("{source}:available"),
            None => "uia_focused_value:unavailable".to_owned(),
        });
        text_readback_attempts.push(cdp_readback.attempt.clone());
        text_readback_attempts.push(ocr_readback.attempt.clone());
        let (value, readback_source) = choose_act_type_text_readback(
            focused.as_ref(),
            uia_value,
            uia_readback_source,
            &cdp_readback,
            &ocr_readback,
        );
        let signature = ActTypeTextSignature {
            foreground_hwnd: input.foreground.hwnd,
            foreground_pid: input.foreground.pid,
            foreground_process: input.foreground.process_name,
            foreground_title_sha256: non_empty_sha256(&input.foreground.window_title),
            focused_element_id: focused.as_ref().map(|item| item.element_id.clone()),
            focused_role: focused.as_ref().map(|item| item.role.clone()),
            focused_name_sha256: focused
                .as_ref()
                .and_then(|item| non_empty_sha256(&item.name)),
            focused_value_len: value.as_ref().map(|value| value.chars().count()),
            focused_value_sha256: value.as_deref().and_then(non_empty_sha256),
            focused_selected_text_sha256: focused
                .as_ref()
                .and_then(|item| item.selected_text.as_deref())
                .and_then(non_empty_sha256),
            focused_bbox: focused.as_ref().map(|item| item.bbox),
            readback_source,
            has_text_readback: value.is_some(),
            text_readback_attempts,
            cdp_status: input.cdp.as_ref().map(|cdp| cdp.status.as_str().to_owned()),
            cdp_endpoint_present: input.cdp.as_ref().is_some_and(|cdp| cdp.endpoint.is_some()),
            cdp_selected_target_id: cdp_readback.target_id.clone().or_else(|| {
                input
                    .cdp
                    .as_ref()
                    .and_then(|cdp| cdp.selected_target_id.clone())
            }),
            cdp_active_has_element: cdp_readback.has_active_element,
            cdp_active_is_editable: cdp_readback.is_editable,
            cdp_active_tag_name: cdp_readback.tag_name,
            cdp_active_input_type: cdp_readback.input_type,
            cdp_active_element_path: cdp_readback.element_path,
            cdp_active_id_sha256: cdp_readback.id_sha256,
            cdp_active_name_sha256: cdp_readback.name_sha256,
            cdp_active_value_len: cdp_readback.value_len,
            cdp_active_value_sha256: cdp_readback.value_sha256,
            cdp_active_error_code: cdp_readback.error_code,
            cdp_active_error_detail_sha256: cdp_readback.error_detail_sha256,
            ocr_word_count: ocr_readback.word_count,
            ocr_text_len: ocr_readback.value_len,
            ocr_text_sha256: ocr_readback.value_sha256,
            web_path: input.web_path.map(|path| path.as_str().to_owned()),
            browser_url_len: browser_url.url.as_ref().map(|url| url.chars().count()),
            browser_url_sha256: browser_url.url.as_deref().and_then(non_empty_sha256),
            browser_cdp_target_id: browser_url.target_id.clone(),
            browser_url_readback_source: browser_url.source,
            browser_title_sha256: bridge_target
                .as_ref()
                .and_then(|readback| readback.title.as_deref())
                .and_then(non_empty_sha256),
            browser_ready_state: bridge_target
                .as_ref()
                .and_then(|readback| readback.ready_state.clone()),
            browser_tab_active: bridge_target.as_ref().and_then(|readback| readback.active),
        };
        if require_focused_text_value && value.is_none() {
            let signature_hash = verify_hash_json(&signature)?;
            return Err(act_type_verify_surface_unavailable_error(
                "no UIA Value/Text, CDP active-element, or focused-rectangle OCR text readback was available for act_type verify_delta",
                signature_hash,
                signature,
            ));
        }

        let date_parent = focused.as_ref().and_then(|item| {
            act_type_date_parent::focused_date_parent_readback(
                &item.element_id,
                &item.role,
                &input.elements,
            )
        });
        Ok(ActTypeTextReadback {
            signature,
            value,
            browser_url: browser_url.url,
            date_parent,
        })
    }

    fn ensure_act_type_foreground_fallback_target(
        &self,
        preflight: &ActionPreflightReadback,
        target: &ActTypeForegroundFallbackTarget,
        recording: Option<&Arc<RecordingBackend>>,
    ) -> Result<(), ErrorData> {
        if recording.is_some() {
            return Err(act_type_foreground_fallback_recording_error(target));
        }
        let expected = preflight.after.as_ref().unwrap_or(&preflight.before);
        if expected.hwnd != target.root_hwnd {
            return Err(act_type_foreground_fallback_target_not_foreground_error(
                expected, target,
            ));
        }
        Ok(())
    }

    async fn click_act_type_foreground_fallback_target(
        handle: ActionHandle,
        target: &ActTypeForegroundFallbackTarget,
    ) -> Result<(), ErrorData> {
        let point = act_type_target_center_point(target)?;
        let actions = [
            Action::MouseMove {
                to: MouseTarget::Screen { point },
                curve: AimCurve::Instant,
                duration_ms: ACT_TYPE_FOREGROUND_FALLBACK_CLICK_DURATION_MS,
                backend: Backend::Auto,
            },
            Action::MouseButton {
                button: MouseButton::Left,
                action: ButtonAction::Press,
                hold_ms: ACT_TYPE_FOREGROUND_FALLBACK_CLICK_HOLD_MS,
                backend: Backend::Auto,
            },
        ];
        for action in actions {
            handle
                .execute(action)
                .await
                .map_err(|error| act_type_foreground_fallback_click_error(target, point, &error))?;
        }
        tracing::info!(
            code = "M2_ACT_TYPE_CHROMIUM_FOREGROUND_FALLBACK_CLICKED",
            element_id = %target.element_id,
            root_hwnd = target.root_hwnd,
            screen_x = point.x,
            screen_y = point.y,
            role = %target.role,
            "readback=foreground_click tool=act_type into_element_fallback=chromium_uia_value_pattern_refused"
        );
        Ok(())
    }

    async fn act_set_field_text_background_guarded(
        params: &crate::m2::ActSetFieldTextParams,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<ForegroundGuardedResponse<crate::m2::ActSetFieldTextResponse>, ErrorData> {
        let element_id = crate::m2::required_element_id(params)?;
        let target = act_set_value_target_foreground_guard(element_id)?;
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?;
        let foreground_section = BackgroundForegroundSection::enter(Some(target)).await;
        with_background_foreground_guard(
            "act_set_field_text",
            session_id.as_deref().unwrap_or("stdio"),
            foreground_section,
            "act_set_field_text.background_tier",
            crate::m2::act_set_field_text_native(params),
            |response| {
                Some(BackgroundGuardReply {
                    required_foreground: &mut response.required_foreground,
                    source_of_truth: Some(response.source_of_truth.as_str()),
                    backend_tier_used: &response.backend_tier_used,
                })
            },
        )
        .await
    }

    /// Leased foreground tier for Chromium UIA editable targets: click the
    /// element, prove focus landed on it, Ctrl+A, type (or Delete for empty
    /// text), then a separate UIA value readback must equal the requested
    /// text. The target window must already be foreground — Synapse never
    /// activates it implicitly (epic #771).
    async fn act_set_field_text_foreground_tier(
        &self,
        params: &crate::m2::ActSetFieldTextParams,
        root_hwnd: i64,
        process_name: &str,
        metadata: synapse_a11y::ElementMetadataReadback,
        preflight: &ActionPreflightReadback,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<crate::m2::ActSetFieldTextResponse, ErrorData> {
        let started = Instant::now();
        let element_id = crate::m2::required_element_id(params)?.clone();
        let target = ActTypeForegroundFallbackTarget {
            element_id: element_id.to_string(),
            root_hwnd,
            process_name: process_name.to_owned(),
            role: metadata.role.clone(),
            automation_id_present: metadata.automation_id.is_some(),
            bbox: metadata.bbox,
            enabled: metadata.enabled,
            keyboard_focusable: metadata.keyboard_focusable,
            patterns: metadata.patterns.clone(),
            name_len: metadata.name.chars().count(),
            value_len: metadata.value.as_ref().map(|value| value.chars().count()),
        };
        // #50: this tier delivers the text through the global input emitter
        // (click + select-all + type), so the act_type foreground
        // monopolization budget applies here too — and the refusal must land
        // before any lease/scroll/click/select-all delivery work.
        crate::m2::validate_foreground_type_text_budget(
            &params.text,
            false,
            &set_field_foreground_replace_dynamics(),
        )?;
        let (handle, recording, _connection_closed_cancel) =
            self.m2_action_context_for_request(request_context)?;
        if recording.is_some() {
            return Err(set_field_text_foreground_error(
                &target,
                error_codes::ACTION_BACKEND_UNAVAILABLE,
                "foreground_tier_recording_backend_unsupported",
                "act_set_field_text Chromium foreground tier requires the live foreground input tier and cannot run against the recording backend",
            ));
        }
        let handle = handle.with_delivery_target(Some(
            self.resolve_input_delivery_target(Some(root_hwnd), &handle)?,
        ));
        let expected = preflight.after.as_ref().unwrap_or(&preflight.before);
        if expected.hwnd != target.root_hwnd {
            return Err(set_field_text_foreground_error(
                &target,
                error_codes::ACTION_FOREGROUND_LOST,
                "target_window_not_foreground",
                format!(
                    "act_set_field_text Chromium foreground tier requires target hwnd 0x{:x} to be the current foreground hwnd, but preflight foreground was 0x{:x}; call act_focus_window first — Synapse never activates a window implicitly",
                    target.root_hwnd, expected.hwnd
                ),
            ));
        }

        let before = synapse_a11y::element_value(&element_id).map_err(|error| {
            set_field_text_foreground_error(
                &target,
                error.code(),
                "before_value_read_failed",
                format!(
                    "act_set_field_text before-value UIA readback failed for element {element_id}: {error}"
                ),
            )
        })?;

        let _lease_guard =
            acquire_tool_foreground_input_lease(self, "act_set_field_text", request_context)?;

        // Actionability before the coordinate click (the Playwright `fill`
        // discipline), shared with act_type's Chromium fallback (#194): scroll an
        // off-viewport target into view, re-read its live bbox, and require the click
        // point to land inside the target window - a stale below-the-fold bbox must
        // never steer a foreground click into another window.
        let mut target = target;
        let point = foreground_click_actionability::actionable_click_point(
            &element_id,
            root_hwnd,
            &metadata.patterns,
            &mut target.bbox,
        )
        .map_err(|refusal| {
            set_field_text_foreground_error(
                &target,
                refusal.code,
                refusal.reason,
                format!("act_set_field_text {}", refusal.detail),
            )
        })?;
        let click_actions = [
            Action::MouseMove {
                to: MouseTarget::Screen { point },
                curve: AimCurve::Instant,
                duration_ms: ACT_TYPE_FOREGROUND_FALLBACK_CLICK_DURATION_MS,
                backend: Backend::Auto,
            },
            Action::MouseButton {
                button: MouseButton::Left,
                action: ButtonAction::Press,
                hold_ms: ACT_TYPE_FOREGROUND_FALLBACK_CLICK_HOLD_MS,
                backend: Backend::Auto,
            },
        ];
        for action in click_actions {
            handle.execute(action).await.map_err(|error| {
                set_field_text_foreground_error(
                    &target,
                    error.code(),
                    "target_click_failed",
                    format!(
                        "act_set_field_text foreground click at ({}, {}) failed: {error}",
                        point.x, point.y
                    ),
                )
            })?;
        }
        tracing::info!(
            code = "M2_ACT_SET_FIELD_TEXT_FOREGROUND_CLICKED",
            element_id = %target.element_id,
            root_hwnd = target.root_hwnd,
            screen_x = point.x,
            screen_y = point.y,
            role = %target.role,
            "readback=foreground_click tool=act_set_field_text tier=foreground_keys"
        );

        let focus_readback = self
            .capture_act_type_text_signature(160, false, false, None)
            .await?;
        act_type_foreground_fallback_focus_matches_target(&target, &focus_readback.signature)?;

        // #188: Ctrl+A and the replacement are real keyboard input. Each chunk is
        // refused unless the preflight-verified target root is the foreground,
        // and every departure from it while a chunk runs raises the interrupt.
        let select_all = crate::m2::select_all_chord_action(60, Backend::Auto)?;
        crate::m2::foreground_type_monitor::execute_on_foreground_target(
            &handle,
            select_all,
            target.root_hwnd,
        )
        .await
        .map_err(|error| {
            set_field_text_foreground_error(
                &target,
                error.code(),
                "select_all_failed",
                format!("act_set_field_text Ctrl+A select-all failed: {error}"),
            )
        })?;

        let (method, replace_action) = if params.text.is_empty() {
            (
                crate::m2::METHOD_FOREGROUND_CLEAR,
                crate::m2::delete_key_action(40, Backend::Auto)?,
            )
        } else {
            (
                crate::m2::METHOD_FOREGROUND_REPLACE,
                Action::TypeText {
                    text: params.text.clone(),
                    dynamics: set_field_foreground_replace_dynamics(),
                    backend: Backend::Auto,
                },
            )
        };
        crate::m2::foreground_type_monitor::execute_on_foreground_target(
            &handle,
            replace_action,
            target.root_hwnd,
        )
        .await
        .map_err(|error| {
            set_field_text_foreground_error(
                &target,
                error.code(),
                "replacement_input_failed",
                format!("act_set_field_text foreground replacement input failed: {error}"),
            )
        })?;

        tokio::time::sleep(Duration::from_millis(u64::from(params.verify_timeout_ms))).await;
        let after = synapse_a11y::element_value(&element_id).map_err(|error| {
            set_field_text_foreground_error(
                &target,
                error.code(),
                "after_value_read_failed",
                format!(
                    "act_set_field_text after-value UIA readback failed for element {element_id}: {error}"
                ),
            )
        })?;

        if before.is_password || after.is_password {
            return set_field_text_password_response(params, started, method, &before, &after);
        }
        crate::m2::finish_replace_response(
            params,
            started,
            method,
            crate::m2::TIER_FOREGROUND_KEYS,
            true,
            crate::m2::SOURCE_UIA_VALUE,
            &before.value,
            &after.value,
            json!({
                "element_id": element_id.to_string(),
                "root_hwnd": target.root_hwnd,
                "process_name": target.process_name,
                "role": target.role,
                "click_point": { "x": point.x, "y": point.y },
                "focused_element_id": focus_readback.signature.focused_element_id,
            }),
        )
    }

    async fn capture_action_delta_signature(
        &self,
        max_elements: usize,
        point_region: Option<Point>,
        include_cursor: bool,
        target_window_hwnd: Option<i64>,
    ) -> Result<ClickDeltaSignature, ErrorData> {
        let mut input = {
            let state = self.m1_state()?;
            if let Some(hwnd) = target_window_hwnd {
                crate::m1::observe_input(
                    &state,
                    &crate::m1::ObserveParams {
                        depth: Some(6),
                        max_elements: Some(max_elements),
                        window_hwnd: Some(hwnd),
                        ..crate::m1::ObserveParams::default()
                    },
                    None,
                )?
            } else {
                crate::m1::current_input(&state, 6)?
            }
        };
        crate::m1::enrich_input_with_cdp(&mut input, 6, max_elements).await;
        crate::m1::enrich_input_with_browser_ocr(&mut input, max_elements);

        let focused = input.focused.clone();
        let elements_sha256 = elements_fingerprint_hash(&input.elements, true)?;
        let effect_elements_sha256 = elements_fingerprint_hash(&input.elements, false)?;
        let foreground_title_sha256 = non_empty_sha256(&input.foreground.window_title);
        // #191: window-scoped capture of the observed window (the target when one is
        // given), with the point region cropped from the same frame.
        let pixel_evidence = pixel_signature::capture_window_pixel_evidence(
            input.foreground.hwnd,
            input.foreground.window_bounds,
            point_region,
        );
        Ok(ClickDeltaSignature {
            foreground_hwnd: input.foreground.hwnd,
            foreground_pid: input.foreground.pid,
            foreground_process: input.foreground.process_name,
            foreground_title: input.foreground.window_title,
            foreground_title_sha256,
            focused_element_id: focused.as_ref().map(|item| item.element_id.to_string()),
            focused_role: focused.as_ref().map(|item| item.role.clone()),
            focused_name_sha256: focused
                .as_ref()
                .and_then(|item| non_empty_sha256(&item.name)),
            focused_value_sha256: focused
                .as_ref()
                .and_then(|item| item.value.as_deref())
                .and_then(non_empty_sha256),
            focused_bbox: focused.as_ref().map(|item| item.bbox),
            element_count: input.elements.len(),
            elements_sha256,
            effect_elements_sha256,
            cdp_status: input.cdp.as_ref().map(|cdp| cdp.status.as_str().to_owned()),
            cdp_endpoint_present: input.cdp.as_ref().is_some_and(|cdp| cdp.endpoint.is_some()),
            web_path: input.web_path.map(|path| path.as_str().to_owned()),
            cursor_position: include_cursor
                .then(|| synapse_action::backend::software::cursor_position().ok())
                .flatten(),
            pixel: pixel_evidence.window,
            point_pixel: pixel_evidence.point,
        })
    }

    async fn chrome_bridge_session_target_readback(
        &self,
        session_id: Option<&str>,
        require_readback: bool,
    ) -> Result<Option<BrowserTargetReadback>, ErrorData> {
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        let Some(SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        }) = self.session_target(Some(session_id))?
        else {
            return Ok(None);
        };
        if synapse_a11y::endpoint_for_window(window_hwnd).is_some() {
            return Ok(None);
        }
        if !is_chrome_bridge_target_id(&cdp_target_id) {
            if require_readback {
                return Err(act_type_browser_url_readback_error(
                    error_codes::A11Y_CDP_ATTACH_FAILED,
                    format!(
                        "act_type readback requires session target {cdp_target_id:?}, but no raw CDP endpoint or normal Chrome bridge target id is available"
                    ),
                    Some("session_target_without_cdp_endpoint"),
                    None,
                    None,
                ));
            }
            return Ok(None);
        }
        let expected_context = synapse_a11y::foreground_context(window_hwnd).ok();
        let info = match crate::chrome_debugger_bridge::target_info(
            window_hwnd,
            &cdp_target_id,
            None,
            expected_context
                .as_ref()
                .map(|context| context.window_bounds),
            expected_context
                .as_ref()
                .map(|context| context.window_title.as_str()),
        )
        .await
        {
            Ok(info) => info,
            Err(error) => {
                if require_readback {
                    return Err(act_type_browser_url_readback_error(
                        error.code(),
                        format!(
                            "Chrome bridge targetInfo readback failed for act_type session target {cdp_target_id:?}: {}",
                            error.detail()
                        ),
                        Some("chrome_tabs_extension"),
                        None,
                        Some(error.detail()),
                    ));
                }
                return Ok(None);
            }
        };
        let source = if info.readback_backend.trim().is_empty() {
            "chrome.tabs.get".to_owned()
        } else {
            info.readback_backend.clone()
        };
        Ok(Some(BrowserTargetReadback {
            url: CdpTargetUrlReadback {
                url: (!info.url.trim().is_empty()).then(|| info.url.clone()),
                target_id: Some(info.target_id.clone()),
                source: Some(source),
            },
            title: (!info.title.trim().is_empty()).then_some(info.title),
            ready_state: (!info.ready_state.trim().is_empty()).then_some(info.ready_state),
            active: Some(info.active),
            active_text: info
                .active_element
                .as_ref()
                .map(|active| chrome_bridge_active_text_readback(&info.target_id, active)),
        }))
    }

    async fn act_type_chrome_bridge_session_target(
        &self,
        session_id: &str,
        params: &ActTypeParams,
        emitted: &str,
    ) -> Result<Option<ActTypeResponse>, ErrorData> {
        if params.into_element.is_some() {
            return Ok(None);
        }
        let Some((window_hwnd, cdp_target_id)) =
            self.chrome_bridge_active_session_target(session_id)?
        else {
            return Ok(None);
        };
        if params.expected_browser_url_regex.is_some() {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "act_type expected_browser_url_regex is a navigation postcondition; use cdp_navigate_tab for session-owned Chrome bridge tabs instead of keyboard typing",
            ));
        }
        if params.backend == TypeBackend::Hardware {
            return Err(mcp_error(
                error_codes::ACTION_BACKEND_UNAVAILABLE,
                "act_type backend=hardware cannot target an inactive Chrome bridge tab; use backend=auto/software for background DOM typing",
            ));
        }
        action_from_type_params(params)?;
        let chars_typed = u32::try_from(emitted.chars().count()).map_err(|_error| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "act_type text has more than u32::MAX chars",
            )
        })?;
        let started = Instant::now();
        let readback =
            crate::chrome_debugger_bridge::type_active_element(window_hwnd, &cdp_target_id, emitted)
                .await
                .map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!(
                            "act_type Chrome bridge active-element typing failed for session target {cdp_target_id:?}: {}",
                            error.detail()
                        ),
                    )
                })?;
        let postcondition =
            act_type_chrome_bridge_type_postcondition(params, emitted, chars_typed, &readback)?;
        tracing::info!(
            code = "M2_ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT",
            session_id = %session_id,
            hwnd = window_hwnd,
            cdp_target_id = %readback.target_id,
            tab_id = readback.tab_id,
            chars_typed = readback.chars_typed,
            readback_backend = %readback.readback_backend,
            target_candidate_count = readback.target_candidate_count,
            target_selection_reason = %readback.target_selection_reason,
            "readback=chrome_bridge_active_element tool=act_type method=chrome.scripting.executeScript"
        );
        Ok(Some(ActTypeResponse {
            ok: true,
            chars_typed,
            elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
            backend_tier_used: ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_TIER.to_owned(),
            required_foreground: false,
            target_text_integrity: ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_TEXT_INTEGRITY.to_owned(),
            target_readback_required: !params.verify_delta,
            minimum_linear_ms_per_char: 20,
            postcondition,
        }))
    }

    fn chrome_bridge_active_session_target(
        &self,
        session_id: &str,
    ) -> Result<Option<(i64, String)>, ErrorData> {
        let target = self.session_target(Some(session_id))?;
        let Some((window_hwnd, cdp_target_id)) =
            chrome_bridge_session_target_parts(target.as_ref())
        else {
            return Ok(None);
        };
        Ok(Some((window_hwnd, cdp_target_id.to_owned())))
    }

    async fn cdp_selected_target_url(
        input: &ObservationInput,
        require_browser_url: bool,
    ) -> Result<CdpTargetUrlReadback, ErrorData> {
        let Some(cdp) = input.cdp.as_ref() else {
            if require_browser_url {
                return Err(act_type_browser_url_readback_error(
                    error_codes::A11Y_CDP_UNREACHABLE,
                    "act_type expected_browser_url_regex requires a Chromium CDP readback, but the foreground is not a CDP-observable browser",
                    None,
                    None,
                    None,
                ));
            }
            return Ok(CdpTargetUrlReadback {
                url: None,
                target_id: None,
                source: None,
            });
        };
        let Some(endpoint) = cdp.endpoint.as_deref() else {
            if require_browser_url {
                return Err(act_type_browser_url_readback_error(
                    cdp.reason_code
                        .as_deref()
                        .unwrap_or(error_codes::A11Y_CDP_UNREACHABLE),
                    "act_type expected_browser_url_regex requires a reachable Chromium CDP endpoint",
                    Some(cdp.status.as_str()),
                    None,
                    cdp.detail.as_deref(),
                ));
            }
            return Ok(CdpTargetUrlReadback {
                url: None,
                target_id: cdp.selected_target_id.clone(),
                source: Some(cdp.status.as_str().to_owned()),
            });
        };
        let Some(target_id) = cdp.selected_target_id.as_deref() else {
            if require_browser_url {
                return Err(act_type_browser_url_readback_error(
                    error_codes::A11Y_CDP_ATTACH_FAILED,
                    "act_type expected_browser_url_regex requires a selected CDP target id from the observation readback",
                    Some(cdp.status.as_str()),
                    Some(endpoint),
                    cdp.detail.as_deref(),
                ));
            }
            return Ok(CdpTargetUrlReadback {
                url: None,
                target_id: None,
                source: Some("cdp_without_selected_target".to_owned()),
            });
        };
        let targets = synapse_a11y::cdp_list_targets(endpoint)
            .await
            .map_err(|error| {
                act_type_browser_url_readback_error(
                    error.code(),
                    format!("Target.getTargets readback failed for act_type browser URL verification: {error}"),
                    Some(cdp.status.as_str()),
                    Some(endpoint),
                    cdp.detail.as_deref(),
                )
            })?;
        let Some(target) = targets
            .iter()
            .find(|target| target.target_id.eq_ignore_ascii_case(target_id))
        else {
            return Err(act_type_browser_url_readback_error(
                error_codes::ACTION_POSTCONDITION_FAILED,
                format!(
                    "Target.getTargets readback did not contain selected target id {target_id:?} for act_type browser URL verification"
                ),
                Some(cdp.status.as_str()),
                Some(endpoint),
                cdp.detail.as_deref(),
            ));
        };
        if target.url.trim().is_empty() && require_browser_url {
            return Err(act_type_browser_url_readback_error(
                error_codes::ACTION_POSTCONDITION_FAILED,
                format!(
                    "Target.getTargets readback for selected target id {target_id:?} contained an empty URL"
                ),
                Some(cdp.status.as_str()),
                Some(endpoint),
                cdp.detail.as_deref(),
            ));
        }
        Ok(CdpTargetUrlReadback {
            url: (!target.url.trim().is_empty()).then(|| target.url.clone()),
            target_id: Some(target.target_id.clone()),
            source: Some("Target.getTargets".to_owned()),
        })
    }

    async fn verify_click_delta(
        &self,
        before: ClickDeltaSignature,
        timeout_ms: u32,
        target_window_hwnd: Option<i64>,
    ) -> Result<ActClickPostcondition, ErrorData> {
        tokio::time::sleep(Duration::from_millis(u64::from(timeout_ms))).await;
        let pixel_only = before.web_path.as_deref() == Some("pixel_only_coordinate");
        let after = if pixel_only {
            self.capture_coordinate_click_delta_signature(before.foreground_hwnd)?
        } else {
            self.capture_action_delta_signature(160, None, false, target_window_hwnd)
                .await?
        };
        let source_of_truth = if pixel_only {
            "target_window_pixels"
        } else {
            click_delta_source_of_truth(target_window_hwnd)
        };
        let before_hash = signature_hash(&before)?;
        let after_hash = signature_hash(&after)?;
        if foreground_identity_changed(&before, &after) {
            return Err(foreground_lost_delta_error(
                "act_click",
                source_of_truth,
                timeout_ms,
                &before_hash,
                &after_hash,
                &before,
                &after,
            ));
        }
        // #107: focus moving alone is not an effect of the click.
        if !click_effect_changed(&before, &after) {
            // #218: the signatures describe the observed (target) window; also report
            // the real OS foreground so a background click is not read as a foreground one.
            let os_foreground_hwnd = synapse_a11y::current_foreground_context()
                .ok()
                .map(|foreground| foreground.hwnd);
            return Err(delivered_no_observed_delta_error(
                "act_click",
                source_of_truth,
                timeout_ms,
                before_hash,
                after_hash,
                click_delta_readback(&before, &after, os_foreground_hwnd),
            ));
        }
        Ok(postcondition_observed_delta(
            "act_click",
            source_of_truth,
            before_hash,
            after_hash,
            "observed a changed focused/UI/pixel signature after delivery",
        ))
    }

    async fn verify_act_type_response(
        &self,
        response: ActTypeResponse,
        before: ActTypeTextReadback,
        verify_timeout_ms: u32,
        emitted: &str,
        browser_url_policy: Option<&ActTypeBrowserUrlPolicy>,
        session_id: Option<&str>,
    ) -> Result<ActTypeResponse, ErrorData> {
        let started = Instant::now();
        let timeout = Duration::from_millis(u64::from(verify_timeout_ms));
        let poll_interval = Duration::from_millis(ACT_TYPE_VERIFY_POLL_INTERVAL_MS);
        let mut last_error: Option<ErrorData> = None;

        loop {
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                break;
            }
            let Some(remaining) = timeout.checked_sub(elapsed) else {
                break;
            };
            tokio::time::sleep(std::cmp::min(poll_interval, remaining)).await;

            let after = self
                .capture_act_type_text_signature(
                    160,
                    false,
                    browser_url_policy.is_some(),
                    session_id,
                )
                .await?;
            let before_hash = verify_hash_json(&before.signature)?;
            let after_hash = verify_hash_json(&after.signature)?;
            let result = if let Some(policy) = browser_url_policy {
                verify_act_type_browser_url_response(
                    response.clone(),
                    before.clone(),
                    after,
                    before_hash,
                    after_hash,
                    verify_timeout_ms,
                    policy,
                )
            } else {
                let terminal_failure = act_type_text_terminal_failure(&before, &after);
                let result = verify_act_type_text_response(
                    response.clone(),
                    before.clone(),
                    after,
                    before_hash,
                    after_hash,
                    verify_timeout_ms,
                    emitted,
                );
                if terminal_failure && result.is_err() {
                    return result;
                }
                result
            };

            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    last_error = Some(error);
                    if started.elapsed() >= timeout {
                        break;
                    }
                }
            }
        }

        let after = match self
            .capture_act_type_text_signature(160, false, browser_url_policy.is_some(), session_id)
            .await
        {
            Ok(after) => after,
            Err(error) => return Err(last_error.unwrap_or(error)),
        };
        let before_hash = verify_hash_json(&before.signature)?;
        let after_hash = verify_hash_json(&after.signature)?;
        if let Some(policy) = browser_url_policy {
            return verify_act_type_browser_url_response(
                response,
                before,
                after,
                before_hash,
                after_hash,
                verify_timeout_ms,
                policy,
            );
        }
        verify_act_type_text_response(
            response,
            before,
            after,
            before_hash,
            after_hash,
            verify_timeout_ms,
            emitted,
        )
    }

    async fn verify_act_type_visual_delta_response(
        &self,
        mut response: ActTypeResponse,
        before: ClickDeltaSignature,
        verify_timeout_ms: u32,
        target_window_hwnd: Option<i64>,
        semantic_error: &ErrorData,
    ) -> Result<ActTypeResponse, ErrorData> {
        let after = self
            .capture_action_delta_signature(160, None, false, target_window_hwnd)
            .await?;
        let source_of_truth = click_delta_source_of_truth(target_window_hwnd);
        // #218: the typed-value readback failed, so delivery is not proven.
        let postcondition = verify_captured_unproven_action_delta(
            "act_type",
            source_of_truth,
            verify_timeout_ms,
            before,
            after,
            None,
            ForegroundChangePolicy::reject(),
        )?;
        tracing::info!(
            code = "M2_ACT_TYPE_VISUAL_DELTA_VERIFIED",
            tool = "act_type",
            target_window_hwnd,
            source_of_truth,
            semantic_error_code = click_error_data_code(semantic_error)
                .unwrap_or(error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE),
            semantic_error_detail = %semantic_error.message,
            "act_type semantic text readback could not prove the delivered input; visual target-window SoT changed after delivery"
        );
        response.postcondition = postcondition;
        response.target_readback_required = false;
        response.target_text_integrity = visual_delta_text_integrity(source_of_truth);
        Ok(response)
    }

    async fn verify_act_press_response(
        &self,
        mut response: ActPressResponse,
        before: ClickDeltaSignature,
        verify_timeout_ms: u32,
        foreground_change_policy: ForegroundChangePolicy,
        native_window_readback: bool,
    ) -> Result<ActPressResponse, ErrorData> {
        response.postcondition = self
            .verify_action_delta(
                "act_press",
                if native_window_readback {
                    "foreground_related_window"
                } else {
                    "foreground_focused_ui_or_pixels"
                },
                before,
                verify_timeout_ms,
                DeltaRegion::default(),
                foreground_change_policy,
            )
            .await?;
        Ok(response)
    }

    async fn try_act_press_background_target(
        &self,
        params: ActPressParams,
        recording_active: bool,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<BackgroundPress, ErrorData> {
        if !press_background_target_candidate(&params, recording_active) {
            return Ok(BackgroundPress::NotTaken);
        }
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?;
        if params.force_foreground_key_delivery {
            // Only the audited `act operation=foreground` wrapper can set this
            // private marker, after its own exact-claim, lease, and native-focus
            // checks. Do not downgrade that chord to PostMessage/CDP or make it
            // wait on a broad UIA snapshot of a WebView provider. In particular,
            // a caller merely using the BreakGlass profile keeps normal
            // target-scoped background keyboard routing.
            return Ok(BackgroundPress::NotTaken);
        }
        let Some(target) = self.action_session_target_override(
            params.window_hwnd,
            params.cdp_target_id.as_deref(),
            session_id.as_deref(),
        )?
        else {
            return Ok(BackgroundPress::NotTaken);
        };
        match target {
            SessionTarget::Cdp {
                window_hwnd,
                cdp_target_id,
            } => self
                .act_press_cdp_background_target(window_hwnd, cdp_target_id, params)
                .await
                .map(|response| BackgroundPress::Delivered(Box::new(response))),
            SessionTarget::Window { hwnd } => {
                let explicit_hwnd = params.window_hwnd;
                let root_hwnd = if let Some(requested) = explicit_hwnd {
                    crate::m2::live_target_root(requested, "act_press window_hwnd check")?
                } else {
                    hwnd
                };
                if explicit_hwnd.is_some() {
                    self.ensure_explicit_input_target_claim(
                        "act_press",
                        Some(&SessionTarget::Window { hwnd: root_hwnd }),
                        request_context,
                    )?;
                }
                // #2340 part B: pick the receiving control once; the route, the
                // delivery and both reads of the check use it.
                let receiver = hwnd_keyboard_target_state(root_hwnd)?;
                #[cfg(windows)]
                let claimed_key_press = {
                    use crate::m2::press::{KeyChord, KeyLane, KeyRoute};
                    let labels = act_press_normalized_labels(&params)?;
                    // A read that fails counts as "not in front": a plain key
                    // or an edit command is still delivered as before, and a
                    // chord is refused honestly.
                    let claimed_and_in_front = session_id.as_deref().is_some_and(|session_id| {
                        matches!(
                            self.claimed_window_in_front(session_id, root_hwnd),
                            Ok(true)
                        )
                    });
                    match KeyRoute::for_window(
                        &KeyChord::of(&labels),
                        &receiver.class_name,
                        claimed_and_in_front,
                    )
                    .lane()
                    {
                        KeyLane::WindowMessages | KeyLane::EditMessage => false,
                        KeyLane::ClaimedKeyPress => true,
                        KeyLane::RefusedNeedsFront => {
                            return Err(mcp_error(
                                error_codes::TOOL_PROFILE_POLICY_DENIED,
                                format!(
                                    "act_press refused before delivery: this chord of {} keys holds Ctrl or Alt, which window messages cannot hold down, so it needs the session's claimed window in front; nothing was sent. Put the claimed window in front, or use act operation=foreground.",
                                    labels.len()
                                ),
                            ));
                        }
                        KeyLane::RefusedSystemChord => {
                            return Err(mcp_error(
                                error_codes::TOOL_PARAMS_INVALID,
                                format!(
                                    "act_press refused before delivery: this chord of {} keys is a system chord (the Win key, Alt+Tab, Alt+Esc, Ctrl+Esc, Ctrl+Alt+Delete or Ctrl+Shift+Esc), which is never sent on a target-scoped route; nothing was sent. Use act operation=foreground.",
                                    labels.len()
                                ),
                            ));
                        }
                    }
                };
                #[cfg(not(windows))]
                let claimed_key_press = false;
                if claimed_key_press {
                    return Ok(BackgroundPress::ClaimedKeyPress {
                        window_hwnd: root_hwnd,
                    });
                }
                self.act_press_postmessage_background_target(
                    root_hwnd,
                    receiver,
                    params,
                    |root, deliver| {
                        if let Some(requested) = explicit_hwnd {
                            // Capture and authorization share the initial resolution.
                            // Re-read the original HWND immediately before posting.
                            let mut initial_root = Some(root);
                            self.dispatch_explicit_press_for_session_with(
                                requested,
                                session_id.as_deref(),
                                |original| {
                                    if let Some(initial) = initial_root.take() {
                                        return Ok(initial);
                                    }
                                    crate::m2::live_target_root(
                                        original,
                                        "act_press window_hwnd check",
                                    )
                                },
                                deliver,
                            )
                        } else {
                            deliver(root)
                        }
                    },
                )
                .await
                .map(|response| BackgroundPress::Delivered(Box::new(response)))
            }
        }
    }

    async fn try_act_keymap_background_target(
        &self,
        resolved: &ResolvedKeymapPress,
        recording_active: bool,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<Option<ActKeymapResponse>, ErrorData> {
        self.try_act_press_background_target(
            resolved.press.clone(),
            recording_active,
            request_context,
        )
        .await
        .map(|route| match route {
            BackgroundPress::Delivered(response) => {
                Some(act_keymap_response_from_press(resolved, *response))
            }
            // act_keymap keeps its own foreground lane (no claimed lease).
            BackgroundPress::NotTaken | BackgroundPress::ClaimedKeyPress { .. } => None,
        })
    }

    async fn try_act_stroke_cdp_background_target(
        &self,
        route: &ActStrokeRoute,
        params: &ActStrokeParams,
        plan: &ActStrokePlan,
    ) -> Result<Option<ActStrokeResponse>, ErrorData> {
        let (window_hwnd, cdp_target_id) = match route {
            ActStrokeRoute::Screen => return Ok(None),
            ActStrokeRoute::CdpTargetRefused => {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "act_stroke active CDP targets require an explicit path for background mouse strokes; refusing to fall back to the real cursor for a browser target",
                ));
            }
            ActStrokeRoute::CdpTarget {
                window_hwnd,
                cdp_target_id,
            } => (*window_hwnd, cdp_target_id),
        };
        let endpoint = synapse_a11y::endpoint_for_window(window_hwnd).ok_or_else(|| {
            mcp_error(
                error_codes::A11Y_CDP_UNREACHABLE,
                format!(
                    "act_stroke background CDP target requires a reachable raw CDP endpoint for window_hwnd {window_hwnd:#x}; the normal chrome.tabs bridge cannot dispatch Input.dispatchMouseEvent"
                ),
            )
        })?;
        act_stroke_cdp_target(&endpoint, cdp_target_id, params.clone(), plan.clone())
            .await
            .map(Some)
    }

    /// Reads the session target and decides the `act_stroke` route once (#202).
    fn act_stroke_route_for_request(
        &self,
        params: &ActStrokeParams,
        plan: &ActStrokePlan,
        recording_active: bool,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<ActStrokeRoute, ErrorData> {
        if recording_active || params.requests_hardware_backend() {
            return Ok(act_stroke_route(None, plan, true));
        }
        let session_target =
            match super::context::mcp_session_id_from_request_context(request_context)? {
                Some(session_id) => self.session_target(Some(&session_id))?,
                None => None,
            };
        Ok(act_stroke_route(session_target, plan, false))
    }

    async fn act_press_cdp_background_target(
        &self,
        window_hwnd: i64,
        cdp_target_id: String,
        params: ActPressParams,
    ) -> Result<ActPressResponse, ErrorData> {
        let endpoint = synapse_a11y::endpoint_for_window(window_hwnd).ok_or_else(|| {
            mcp_error(
                error_codes::A11Y_CDP_UNREACHABLE,
                format!(
                    "act_press background CDP target requires a reachable CDP endpoint for window_hwnd {window_hwnd:#x}"
                ),
            )
        })?;
        let before = if params.verify_delta {
            Some(
                self.capture_cdp_keyboard_delta_signature(&endpoint, &cdp_target_id)
                    .await?,
            )
        } else {
            None
        };
        let verify_timeout_ms = params.verify_timeout_ms;
        let mut response = act_press_cdp_target(&endpoint, &cdp_target_id, params).await?;
        if let Some(before) = before {
            tokio::time::sleep(Duration::from_millis(u64::from(verify_timeout_ms))).await;
            let after = self
                .capture_cdp_keyboard_delta_signature(&endpoint, &cdp_target_id)
                .await?;
            response.postcondition = verify_keyboard_delta_signature(
                "act_press",
                "cdp_active_element_value_or_selection",
                verify_timeout_ms,
                before,
                after,
                "observed CDP target active-element value/selection change after Input.dispatchKeyEvent delivery",
            )?;
        }
        Ok(response)
    }

    async fn act_press_postmessage_background_target(
        &self,
        root_hwnd: i64,
        receiver: HwndKeyboardTargetState,
        params: ActPressParams,
        dispatch: impl FnOnce(
            i64,
            &mut dyn FnMut(i64) -> Result<(), ErrorData>,
        ) -> Result<(), ErrorData>,
    ) -> Result<ActPressResponse, ErrorData> {
        let expected_effect = hwnd_keyboard_expected_effect(&params)?;
        // #2340 part B: `receiver` is the control the caller picked once; the
        // keys go to it and both reads read it.
        let target_hwnd = receiver.hwnd;
        let before = HwndKeyboardDeltaSignature {
            target: receiver,
            clipboard_sequence: crate::m2::press::clipboard_sequence_number(),
        };
        // #371: a key-binding window (winit, games) never changes its window
        // text, so its delivery can only be read from the window's pixels.
        #[cfg(windows)]
        let text_control = crate::m2::press::is_text_control_class(&before.target.class_name);
        #[cfg(not(windows))]
        let text_control = true;
        let before_visual = if params.verify_delta || !text_control {
            Some(self.capture_coordinate_click_delta_signature(root_hwnd)?)
        } else {
            None
        };
        let verify_timeout_ms = params.verify_timeout_ms;
        let mut response =
            act_press_postmessage_target(root_hwnd, target_hwnd, params, dispatch).await?;
        tokio::time::sleep(Duration::from_millis(u64::from(verify_timeout_ms))).await;
        let after = self.capture_hwnd_keyboard_delta_signature(root_hwnd, target_hwnd)?;
        response.postcondition = match verify_hwnd_keyboard_delta_signature(
            "act_press",
            "target_hwnd_text_or_selection",
            verify_timeout_ms,
            before,
            after,
            expected_effect,
            "observed target HWND text/selection change after PostMessage keyboard delivery",
        ) {
            Ok(postcondition) => postcondition,
            Err(error)
                if click_error_data_code(&error) == Some(error_codes::ACTION_NO_OBSERVED_DELTA) =>
            {
                let Some(before_visual) = before_visual else {
                    return Err(error);
                };
                tracing::info!(
                    code = "M2_ACT_PRESS_VISUAL_DELTA_AFTER_HWND_NO_TEXT_DELTA",
                    tool = "act_press",
                    root_hwnd,
                    semantic_error_code = error_codes::ACTION_NO_OBSERVED_DELTA,
                    semantic_error_detail = %error.message,
                    "act_press target HWND text/selection readback showed no delta; checking target-window UI/pixel SoT"
                );
                let after_visual = self.capture_coordinate_click_delta_signature(root_hwnd)?;
                // #218: a posted key is not proven delivered.
                verify_captured_unproven_action_delta(
                    "act_press",
                    "target_window_ui_or_pixels",
                    verify_timeout_ms,
                    before_visual,
                    after_visual,
                    None,
                    ForegroundChangePolicy::reject(),
                )?
            }
            Err(error) => return Err(error),
        };
        Ok(response)
    }

    async fn capture_cdp_keyboard_delta_signature(
        &self,
        endpoint: &str,
        cdp_target_id: &str,
    ) -> Result<CdpKeyboardDeltaSignature, ErrorData> {
        let state = synapse_a11y::cdp_active_element_state(endpoint, cdp_target_id)
            .await
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!(
                        "act_press CDP active-element Source-of-Truth readback failed for target {cdp_target_id:?}: {error}"
                    ),
                )
            })?;
        Ok(cdp_keyboard_delta_signature(state))
    }

    fn capture_hwnd_keyboard_delta_signature(
        &self,
        root_hwnd: i64,
        target_hwnd: i64,
    ) -> Result<HwndKeyboardDeltaSignature, ErrorData> {
        Ok(HwndKeyboardDeltaSignature {
            target: crate::m2::press::delivered_keyboard_target_state(root_hwnd, target_hwnd)?,
            clipboard_sequence: crate::m2::press::clipboard_sequence_number(),
        })
    }

    async fn verify_act_scroll_response(
        &self,
        mut response: ActScrollResponse,
        before: ClickDeltaSignature,
        verify_timeout_ms: u32,
        region: DeltaRegion,
    ) -> Result<ActScrollResponse, ErrorData> {
        response.postcondition = self
            .verify_action_delta(
                "act_scroll",
                if region.point.is_some() {
                    "target_point_pixels"
                } else {
                    "foreground_focused_ui_or_pixels"
                },
                before,
                verify_timeout_ms,
                region,
                ForegroundChangePolicy::reject(),
            )
            .await?;
        Ok(response)
    }

    async fn verify_act_stroke_response(
        &self,
        mut response: ActStrokeResponse,
        before: ClickDeltaSignature,
        verify_timeout_ms: u32,
    ) -> Result<ActStrokeResponse, ErrorData> {
        response.postcondition = self
            .verify_action_delta(
                "act_stroke",
                "cursor_foreground_ui_or_pixels",
                before,
                verify_timeout_ms,
                DeltaRegion::default(),
                ForegroundChangePolicy::reject(),
            )
            .await?;
        Ok(response)
    }

    async fn verify_action_delta(
        &self,
        tool: &str,
        source_of_truth: &str,
        before: ClickDeltaSignature,
        timeout_ms: u32,
        region: DeltaRegion,
        mut foreground_change_policy: ForegroundChangePolicy,
    ) -> Result<ActPostcondition, ErrorData> {
        let delay = action_delta_verification_delay(source_of_truth, timeout_ms);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let after = if source_of_truth == "foreground_related_window" {
            self.capture_native_foreground_signature()?
        } else if before.web_path.as_deref() == Some("pixel_only_coordinate") {
            self.capture_raw_input_delta_signature(
                region.target_window,
                region.point,
                source_of_truth.contains("cursor"),
            )?
        } else {
            // #191: the window resolved before the action, never re-resolved.
            self.capture_action_delta_signature(
                160,
                region.point,
                source_of_truth.contains("cursor"),
                region.target_window,
            )
            .await?
        };
        if let Some(evidence) = foreground_change_policy.related_window.as_mut() {
            fill_related_window_evidence(evidence, &before, &after);
        }
        verify_captured_action_delta(
            tool,
            source_of_truth,
            timeout_ms,
            before,
            after,
            region.point,
            foreground_change_policy,
        )
    }

    async fn verify_act_pad_response(
        &self,
        mut response: ActPadResponse,
        before: ActionStateSnapshot,
        snapshot_handle: ActionEmitterSnapshotHandle,
        verify_timeout_ms: u32,
    ) -> Result<ActPadResponse, ErrorData> {
        tokio::time::sleep(Duration::from_millis(u64::from(verify_timeout_ms))).await;
        let after = snapshot_handle
            .snapshot()
            .await
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let before_hash = verify_hash_json(&before.pad_state)?;
        let after_hash = verify_hash_json(&after.pad_state)?;
        if before.pad_state == after.pad_state {
            return Err(source_no_observed_delta_error(
                "act_pad",
                "action_emitter.pad_state",
                verify_timeout_ms,
                before_hash,
                after_hash,
                json!({
                    "before_pad_state": before.pad_state,
                    "after_pad_state": after.pad_state,
                }),
            ));
        }
        response.postcondition = postcondition_observed_delta(
            "act_pad",
            "action_emitter.pad_state",
            before_hash,
            after_hash,
            "observed action emitter pad_state change after delivery",
        );
        Ok(response)
    }
}

fn verify_captured_action_delta(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before: ClickDeltaSignature,
    after: ClickDeltaSignature,
    point_region: Option<Point>,
    foreground_change_policy: ForegroundChangePolicy,
) -> Result<ActPostcondition, ErrorData> {
    let check = CapturedDeltaCheck {
        tool,
        source_of_truth,
        timeout_ms,
        delivery: ActionDelivery::Delivered,
    };
    verify_captured_delta(
        &check,
        before,
        after,
        point_region,
        foreground_change_policy,
    )
}

/// `verify_captured_action_delta` for input whose delivery is not proven
/// (#218): a posted key, or act_type after its typed-value readback failed. A
/// no-delta error never claims `delivered: true` here.
fn verify_captured_unproven_action_delta(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before: ClickDeltaSignature,
    after: ClickDeltaSignature,
    point_region: Option<Point>,
    foreground_change_policy: ForegroundChangePolicy,
) -> Result<ActPostcondition, ErrorData> {
    let check = CapturedDeltaCheck {
        tool,
        source_of_truth,
        timeout_ms,
        delivery: ActionDelivery::Unproven,
    };
    verify_captured_delta(
        &check,
        before,
        after,
        point_region,
        foreground_change_policy,
    )
}

/// Whether the input behind a verify_delta check verifiably reached its
/// target (#218).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionDelivery {
    /// SendInput / click paths: the input was delivered.
    Delivered,
    /// Posted keys, or act_type after its typed-value readback failed.
    Unproven,
}

/// The identity of one captured-delta verification (#218).
struct CapturedDeltaCheck<'a> {
    tool: &'a str,
    source_of_truth: &'a str,
    timeout_ms: u32,
    delivery: ActionDelivery,
}

impl CapturedDeltaCheck<'_> {
    /// The no-delta error; `delivered: true` + retry_hint only for delivered input.
    fn no_delta_error(
        &self,
        before_hash: String,
        after_hash: String,
        readback: Value,
    ) -> ErrorData {
        let error = source_no_observed_delta_error(
            self.tool,
            self.source_of_truth,
            self.timeout_ms,
            before_hash,
            after_hash,
            readback,
        );
        match self.delivery {
            ActionDelivery::Delivered => mark_input_delivered(error),
            ActionDelivery::Unproven => error,
        }
    }
}

fn verify_point_pixel_delta(
    check: &CapturedDeltaCheck<'_>,
    point: Point,
    before: ClickDeltaSignature,
    after: ClickDeltaSignature,
) -> Result<ActPostcondition, ErrorData> {
    let before_point = before.point_pixel;
    let after_point = after.point_pixel;
    let before_hash = verify_hash_json(&before_point)?;
    let after_hash = verify_hash_json(&after_point)?;
    // #191: only two successful captures with different pixels are a delta;
    // a capture that failed on either side is unavailable evidence.
    let evidence = click_effect::point_pixel_evidence(before_point.as_ref(), after_point.as_ref());
    if evidence != click_effect::PointPixelEvidence::Changed {
        return Err(check.no_delta_error(
            before_hash,
            after_hash,
            json!({
                "point": point,
                "point_pixel_evidence": evidence.label(),
                "before_point_pixel": before_point,
                "after_point_pixel": after_point,
            }),
        ));
    }
    Ok(postcondition_observed_delta(
        check.tool,
        check.source_of_truth,
        before_hash,
        after_hash,
        "observed a target point pixel signature change after delivery",
    ))
}

fn verify_captured_delta(
    check: &CapturedDeltaCheck<'_>,
    before: ClickDeltaSignature,
    after: ClickDeltaSignature,
    point_region: Option<Point>,
    foreground_change_policy: ForegroundChangePolicy,
) -> Result<ActPostcondition, ErrorData> {
    if let Some(point) = point_region {
        return verify_point_pixel_delta(check, point, before, after);
    }
    let (tool, source_of_truth) = (check.tool, check.source_of_truth);
    let before_hash = signature_hash(&before)?;
    let after_hash = signature_hash(&after)?;
    if foreground_identity_changed(&before, &after) {
        return verify_foreground_transition(
            tool,
            source_of_truth,
            check.timeout_ms,
            before_hash,
            after_hash,
            &before,
            &after,
            &foreground_change_policy,
        );
    }
    let readback = || json!({ "before": before, "after": after });
    if foreground_change_policy.has_expectations() {
        let mut readback = readback();
        readback["foreground_change_policy"] =
            foreground_change_policy_readback(&foreground_change_policy);
        return Err(postcondition_failed_error(
            tool,
            source_of_truth,
            "expected foreground transition did not occur",
            before_hash,
            after_hash,
            readback,
        ));
    }
    if source_of_truth == "foreground_related_window" {
        // The explicit foreground lane historically reports unchanged-window
        // delivery as unverified; pixels alone must not upgrade that result.
        return Ok(postcondition_not_requested(tool, source_of_truth));
    }
    // #191: a capture that failed on one side is no evidence, not a delta.
    if !click_effect::observed_state_changed(&before, &after) {
        return Err(check.no_delta_error(before_hash, after_hash, readback()));
    }
    Ok(postcondition_observed_delta(
        tool,
        source_of_truth,
        before_hash,
        after_hash,
        "observed a Source-of-Truth signature change after delivery",
    ))
}

fn verify_keyboard_delta_signature<T>(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before: T,
    after: T,
    success_detail: &str,
) -> Result<ActPostcondition, ErrorData>
where
    T: Serialize + PartialEq,
{
    let before_hash = hash_json(&before)?;
    let after_hash = hash_json(&after)?;
    if before == after {
        return Err(source_no_observed_delta_error(
            tool,
            source_of_truth,
            timeout_ms,
            before_hash,
            after_hash,
            json!({
                "before": before,
                "after": after,
            }),
        ));
    }
    Ok(postcondition_observed_delta(
        tool,
        source_of_truth,
        before_hash,
        after_hash,
        success_detail,
    ))
}

fn verify_hwnd_keyboard_delta_signature(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before: HwndKeyboardDeltaSignature,
    after: HwndKeyboardDeltaSignature,
    expected_effect: HwndKeyboardExpectedEffect,
    success_detail: &str,
) -> Result<ActPostcondition, ErrorData> {
    let before_hash = hash_json(&before)?;
    let after_hash = hash_json(&after)?;
    if before == after {
        return Err(source_no_observed_delta_error(
            tool,
            source_of_truth,
            timeout_ms,
            before_hash,
            after_hash,
            json!({
                "before": before,
                "after": after,
                "expected_effect": hwnd_keyboard_expected_effect_name(&expected_effect),
            }),
        ));
    }
    if let Some(reason) = hwnd_keyboard_effect_mismatch(&before, &after, &expected_effect) {
        return Err(postcondition_failed_error(
            tool,
            source_of_truth,
            reason,
            before_hash,
            after_hash,
            json!({
                "before": before,
                "after": after,
                "expected_effect": hwnd_keyboard_expected_effect_name(&expected_effect),
            }),
        ));
    }
    Ok(postcondition_observed_delta(
        tool,
        source_of_truth,
        before_hash,
        after_hash,
        success_detail,
    ))
}

fn hwnd_keyboard_expected_effect(
    params: &ActPressParams,
) -> Result<HwndKeyboardExpectedEffect, ErrorData> {
    let labels = act_press_normalized_labels(params)?;
    if labels == ["ctrl", "a"] {
        return Ok(HwndKeyboardExpectedEffect::SelectAll);
    }
    // Ctrl+C / Ctrl+X copy/cut to the clipboard without changing target text
    // (copy) — their effect is a clipboard sequence-number change (#1331).
    if hwnd_is_clipboard_chord(&labels) {
        return Ok(HwndKeyboardExpectedEffect::Clipboard);
    }
    let has_command_modifier = labels
        .iter()
        .any(|label| matches!(label.as_str(), "ctrl" | "alt" | "super"));
    if !has_command_modifier && labels.len() == 1 {
        if let Some(text) = hwnd_printable_text_for_label(&labels[0]) {
            return Ok(HwndKeyboardExpectedEffect::PrintableText { text });
        }
    }
    Ok(HwndKeyboardExpectedEffect::AnyDelta)
}

/// True for a Ctrl+C / Ctrl+X chord (copy/cut): exactly ctrl + {c|x}, no other
/// command modifier. These mutate the clipboard, not the target text (#1331).
fn hwnd_is_clipboard_chord(labels: &[String]) -> bool {
    let has_ctrl = labels.iter().any(|label| label == "ctrl");
    if !has_ctrl {
        return false;
    }
    if labels
        .iter()
        .any(|label| matches!(label.as_str(), "alt" | "super" | "shift"))
    {
        return false;
    }
    let letters: Vec<&str> = labels
        .iter()
        .filter(|label| label.as_str() != "ctrl")
        .map(String::as_str)
        .collect();
    letters.len() == 1 && matches!(letters[0], "c" | "x")
}

fn hwnd_printable_text_for_label(label: &str) -> Option<String> {
    if label.len() == 1 && label.as_bytes()[0].is_ascii_alphanumeric() {
        return Some(label.to_owned());
    }
    match label {
        "`" => Some("`".to_owned()),
        "space" => Some(" ".to_owned()),
        _ => None,
    }
}

fn hwnd_keyboard_effect_mismatch(
    before: &HwndKeyboardDeltaSignature,
    after: &HwndKeyboardDeltaSignature,
    expected_effect: &HwndKeyboardExpectedEffect,
) -> Option<&'static str> {
    match expected_effect {
        HwndKeyboardExpectedEffect::AnyDelta => None,
        HwndKeyboardExpectedEffect::Clipboard => {
            if before.clipboard_sequence == after.clipboard_sequence {
                return Some(
                    "clipboard chord (Ctrl+C/Ctrl+X) did not change the clipboard sequence number",
                );
            }
            None
        }
        HwndKeyboardExpectedEffect::SelectAll => {
            if !same_hwnd_keyboard_target(before, after) {
                return Some("target HWND changed while verifying Ctrl+A select-all delivery");
            }
            if before.target.text_len != after.target.text_len
                || before.target.text_sha256 != after.target.text_sha256
            {
                return Some("Ctrl+A select-all changed target text instead of preserving it");
            }
            if !selection_covers_text(&after.target) {
                return Some("Ctrl+A select-all did not select the full target text");
            }
            None
        }
        HwndKeyboardExpectedEffect::PrintableText { text } => {
            if !same_hwnd_keyboard_target(before, after) {
                return Some("target HWND changed while verifying printable key delivery");
            }
            if before.target.text_sha256 == after.target.text_sha256 {
                return Some("printable key did not change target text");
            }
            if selection_covers_text(&before.target) {
                let expected_len = text.chars().count();
                let Ok(expected_len_u32) = u32::try_from(expected_len) else {
                    return Some("printable key expected text length exceeded u32::MAX");
                };
                let expected_sha256 = text_sha256(text);
                if after.target.text_len != Some(expected_len)
                    || after.target.text_sha256.as_deref() != Some(expected_sha256.as_str())
                    || after.target.selection_start != Some(expected_len_u32)
                    || after.target.selection_end != Some(expected_len_u32)
                {
                    return Some(
                        "printable key after full selection did not replace target text with the emitted character",
                    );
                }
            }
            None
        }
    }
}

fn same_hwnd_keyboard_target(
    before: &HwndKeyboardDeltaSignature,
    after: &HwndKeyboardDeltaSignature,
) -> bool {
    before.target.root_hwnd == after.target.root_hwnd
        && before.target.hwnd == after.target.hwnd
        && before.target.class_name == after.target.class_name
}

fn selection_covers_text(target: &HwndKeyboardTargetState) -> bool {
    let Some(text_len) = target.text_len else {
        return false;
    };
    let Ok(text_len) = u32::try_from(text_len) else {
        return false;
    };
    target.selection_start == Some(0) && target.selection_end == Some(text_len)
}

fn hwnd_keyboard_expected_effect_name(
    expected_effect: &HwndKeyboardExpectedEffect,
) -> &'static str {
    match expected_effect {
        HwndKeyboardExpectedEffect::AnyDelta => "any_delta",
        HwndKeyboardExpectedEffect::PrintableText { .. } => "printable_text",
        HwndKeyboardExpectedEffect::SelectAll => "select_all",
        HwndKeyboardExpectedEffect::Clipboard => "clipboard",
    }
}

fn cdp_keyboard_delta_signature(
    state: synapse_a11y::CdpActiveElementState,
) -> CdpKeyboardDeltaSignature {
    CdpKeyboardDeltaSignature {
        target_id: state.target_id,
        has_active_element: state.has_active_element,
        tag_name: state.tag_name,
        id_sha256: non_empty_sha256(&state.id),
        name_sha256: non_empty_sha256(&state.name),
        value_len: state.value.chars().count(),
        value_sha256: text_sha256(&state.value),
        selection_start: state.selection_start,
        selection_end: state.selection_end,
    }
}

/// What the target-scoped key route did with one press (#2340 part B).
enum BackgroundPress {
    /// Sent on a background lane (CDP or window messages) and checked.
    Delivered(Box<ActPressResponse>),
    /// No background lane applies; the caller's foreground lane runs as before.
    NotTaken,
    /// The session's claimed window is in front: the caller's foreground lane
    /// sends a real key press under the claimed lease of this window.
    ClaimedKeyPress { window_hwnd: i64 },
}

fn press_background_target_candidate(params: &ActPressParams, recording_active: bool) -> bool {
    if recording_active {
        return false;
    }
    if !matches!(params.backend, PressBackend::Auto | PressBackend::Software) {
        return false;
    }
    !params.allow_foreground_change
        && params.expected_foreground_process_regex.is_none()
        && params.expected_foreground_title_regex.is_none()
}

fn act_press_foreground_change_policy(
    params: &ActPressParams,
) -> Result<ForegroundChangePolicy, ErrorData> {
    let has_process_expectation = params.expected_foreground_process_regex.is_some();
    let has_title_expectation = params.expected_foreground_title_regex.is_some();
    let has_expectation = has_process_expectation || has_title_expectation;

    if !params.verify_delta && (params.allow_foreground_change || has_expectation) {
        return Err(act_press_policy_params_invalid(
            "verify_delta",
            "act_press foreground-change verification policy requires verify_delta=true",
            "verify_delta_required",
        ));
    }

    if !params.allow_foreground_change && has_expectation {
        return Err(act_press_policy_params_invalid(
            "allow_foreground_change",
            "expected foreground regex fields require allow_foreground_change=true",
            "allow_foreground_change_required",
        ));
    }

    if !params.allow_foreground_change {
        return Ok(ForegroundChangePolicy::reject());
    }

    let expected_process_pattern = params.expected_foreground_process_regex.clone();
    let expected_title_pattern = params.expected_foreground_title_regex.clone();
    let expected_process_regex = compile_act_press_policy_regex(
        "expected_foreground_process_regex",
        expected_process_pattern.as_deref(),
    )?;
    let expected_title_regex = compile_act_press_policy_regex(
        "expected_foreground_title_regex",
        expected_title_pattern.as_deref(),
    )?;

    Ok(ForegroundChangePolicy {
        allow: true,
        related_window: None,
        expected_process_regex,
        expected_process_pattern,
        expected_title_regex,
        expected_title_pattern,
    })
}

fn act_type_browser_url_policy(
    params: &ActTypeParams,
) -> Result<Option<ActTypeBrowserUrlPolicy>, ErrorData> {
    let Some(pattern) = params.expected_browser_url_regex.as_deref() else {
        return Ok(None);
    };
    if !params.verify_delta {
        return Err(act_type_url_policy_params_invalid(
            "verify_delta",
            "act_type expected_browser_url_regex requires verify_delta=true",
            "verify_delta_required",
        ));
    }
    if params.into_element.is_some() {
        return Err(act_type_url_policy_params_invalid(
            "into_element",
            "act_type expected_browser_url_regex applies only to foreground typing, not into_element routing",
            "foreground_typing_required",
        ));
    }
    if pattern.trim().is_empty() {
        return Err(act_type_url_policy_params_invalid(
            "expected_browser_url_regex",
            "expected_browser_url_regex must not be empty",
            "empty_expected_browser_url_regex",
        ));
    }
    let expected_url_regex = regex::Regex::new(pattern).map_err(|error| {
        act_type_url_policy_params_invalid(
            "expected_browser_url_regex",
            format!("expected_browser_url_regex is not a valid regex: {error}"),
            "invalid_expected_browser_url_regex",
        )
    })?;
    Ok(Some(ActTypeBrowserUrlPolicy {
        expected_url_regex,
        expected_url_pattern: pattern.to_owned(),
    }))
}

fn act_type_should_capture_text_signature(params: &ActTypeParams) -> bool {
    params.verify_delta && params.into_element.is_none()
}

fn act_type_should_capture_visual_signature(
    params: &ActTypeParams,
    target_window_hwnd: Option<i64>,
) -> bool {
    params.verify_delta && params.into_element.is_none() && target_window_hwnd.is_some()
}

fn act_type_visual_delta_target_window(
    params: &ActTypeParams,
    browser_url_policy: Option<&ActTypeBrowserUrlPolicy>,
) -> Result<Option<i64>, ErrorData> {
    let Some(hwnd) = params.verify_target_window_hwnd else {
        return Ok(None);
    };
    if !params.verify_delta {
        return Err(act_type_visual_delta_params_invalid(
            "verify_delta",
            "verify_target_window_hwnd requires verify_delta=true",
            "verify_delta_required",
        ));
    }
    if params.into_element.is_some() {
        return Err(act_type_visual_delta_params_invalid(
            "into_element",
            "verify_target_window_hwnd applies only to foreground typing, not into_element routing",
            "foreground_typing_required",
        ));
    }
    if browser_url_policy.is_some() {
        return Err(act_type_visual_delta_params_invalid(
            "expected_browser_url_regex",
            "verify_target_window_hwnd cannot replace browser URL verification; remove one postcondition",
            "conflicting_postconditions",
        ));
    }
    if hwnd <= 0 {
        return Err(act_type_visual_delta_params_invalid(
            "verify_target_window_hwnd",
            "verify_target_window_hwnd must be a positive top-level HWND",
            "invalid_hwnd",
        ));
    }
    Ok(Some(hwnd))
}

fn act_type_requires_foreground_route(
    params: &ActTypeParams,
    fallback_target: Option<&ActTypeForegroundFallbackTarget>,
) -> bool {
    params.into_element.is_none() || fallback_target.is_some()
}

#[cfg(windows)]
fn act_type_chromium_foreground_fallback_target(
    element_id: Option<&ElementId>,
) -> Result<Option<ActTypeForegroundFallbackTarget>, ErrorData> {
    let Some(element_id) = element_id else {
        return Ok(None);
    };
    if synapse_a11y::cdp_backend_from_element_id(element_id).is_some() {
        return Ok(None);
    }
    let root_hwnd = element_id
        .parts()
        .map_err(|err| {
            mcp_error(
                error_codes::ACTION_ELEMENT_NOT_RESOLVED,
                format!("act_type into_element id is malformed: {err}"),
            )
        })?
        .hwnd;
    let context = synapse_a11y::foreground_context(root_hwnd).map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "act_type into_element foreground fallback target HWND readback failed: {error}"
            ),
        )
    })?;
    if !synapse_a11y::is_chromium_family(&context.process_name) {
        return Ok(None);
    }
    let metadata = synapse_a11y::element_metadata(element_id).map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "act_type into_element foreground fallback target metadata readback failed: {error}"
            ),
        )
    })?;
    if !chromium_editable_value_pattern_requires_foreground_fallback(
        &context.process_name,
        &metadata,
    ) {
        return Ok(None);
    }
    Ok(Some(ActTypeForegroundFallbackTarget {
        element_id: element_id.to_string(),
        root_hwnd,
        process_name: context.process_name,
        role: metadata.role,
        automation_id_present: metadata.automation_id.is_some(),
        bbox: metadata.bbox,
        enabled: metadata.enabled,
        keyboard_focusable: metadata.keyboard_focusable,
        patterns: metadata.patterns,
        name_len: metadata.name.chars().count(),
        value_len: metadata.value.as_ref().map(|value| value.chars().count()),
    }))
}

#[cfg(not(windows))]
fn act_type_chromium_foreground_fallback_target(
    _element_id: Option<&ElementId>,
) -> Result<Option<ActTypeForegroundFallbackTarget>, ErrorData> {
    Ok(None)
}

fn chromium_editable_value_pattern_requires_foreground_fallback(
    process_name: &str,
    metadata: &synapse_a11y::ElementMetadataReadback,
) -> bool {
    if !synapse_a11y::is_chromium_family(process_name) || !metadata.enabled {
        return false;
    }
    if !metadata.patterns.contains(&UiaPattern::Value) {
        return false;
    }
    metadata.keyboard_focusable
        && (act_type_editable_role(&metadata.role) || metadata.patterns.contains(&UiaPattern::Text))
}

fn act_type_editable_role(role: &str) -> bool {
    let role = role.to_ascii_lowercase();
    role.contains("edit") || role.contains("document") || role.contains("text")
}

fn act_type_url_policy_params_invalid(
    field: &'static str,
    detail: impl Into<String>,
    reason: &'static str,
) -> ErrorData {
    let detail = detail.into();
    tracing::error!(
        code = error_codes::TOOL_PARAMS_INVALID,
        tool = "act_type",
        field,
        reason,
        detail = %detail,
        "act_type browser URL policy parameters invalid"
    );
    ErrorData::new(
        ErrorCode(-32099),
        detail.clone(),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "tool": "act_type",
            "field": field,
            "reason": reason,
            "detail": detail,
        })),
    )
}

fn act_type_visual_delta_params_invalid(
    field: &'static str,
    detail: impl Into<String>,
    reason: &'static str,
) -> ErrorData {
    let detail = detail.into();
    tracing::error!(
        code = error_codes::TOOL_PARAMS_INVALID,
        tool = "act_type",
        field,
        reason,
        detail = %detail,
        "act_type visual delta parameters invalid"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!("act_type verify_target_window_hwnd invalid: {detail}"),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "tool": "act_type",
            "field": field,
            "reason": reason,
            "detail": detail,
        })),
    )
}

fn compile_act_press_policy_regex(
    field: &'static str,
    pattern: Option<&str>,
) -> Result<Option<regex::Regex>, ErrorData> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    if pattern.trim().is_empty() {
        return Err(act_press_policy_params_invalid(
            field,
            format!("{field} must not be empty"),
            "empty_expected_foreground_regex",
        ));
    }
    regex::Regex::new(pattern)
        .map(Some)
        .map_err(|error| act_press_policy_invalid_regex(field, pattern, &error))
}

/// How a newly foregrounded window relates to the target window, if it does.
///
/// `same_process_owner_chain`: owned (directly or transitively) by the target
/// window, every link in the target's process. `child_process` (#92): a new
/// window in a descendant process of the target (Tauri/WebView2 DevTools, an
/// unowned top-level window of `msedgewebview2.exe`). Both require a window
/// that was not visible before the action, so Alt+Tab to an existing window and
/// foreground theft by an unrelated process stay refused. A shared shell/broker
/// host as the target proves nothing about its descendants and never relates.
fn related_window_relationship(
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
    evidence: Option<&RelatedWindowEvidence>,
) -> Option<&'static str> {
    let evidence = evidence?;
    if before.foreground_hwnd == after.foreground_hwnd
        || before.foreground_hwnd == 0
        || after.foreground_hwnd == 0
        || before.foreground_pid == 0
        || evidence.prior_windows.contains(&after.foreground_hwnd)
    {
        return None;
    }
    if before.foreground_pid == after.foreground_pid {
        // PID equality alone is unsafe for shared host processes.
        let owned = evidence
            .owner_chain
            .iter()
            .all(|(_, pid)| *pid == before.foreground_pid)
            && evidence
                .owner_chain
                .contains(&(before.foreground_hwnd, before.foreground_pid));
        return owned.then_some("same_process_owner_chain");
    }
    (!crate::m2::process_descendants::is_shared_host_process(&before.foreground_process)
        && evidence.descendant_pids.contains(&after.foreground_pid))
    .then_some("child_process")
}

fn verify_foreground_transition(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before_hash: String,
    after_hash: String,
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
    policy: &ForegroundChangePolicy,
) -> Result<ActPostcondition, ErrorData> {
    if !policy.allow {
        let relationship = matches!(tool, "act_press" | "act_click" | "act_type")
            .then(|| related_window_relationship(before, after, policy.related_window.as_ref()))
            .flatten();
        if let Some(relationship) = relationship {
            let mut postcondition = postcondition_observed_delta(
                tool,
                "foreground_related_window",
                before_hash,
                after_hash,
                "related_window_observed after delivery; temporal observation does not establish that the chord created the window",
            );
            postcondition.related_window_observed =
                Some(crate::m2::postcondition::RelatedWindowObservation {
                    hwnd: after.foreground_hwnd,
                    pid: after.foreground_pid,
                    title: after.foreground_title.clone(),
                    target_hwnd: before.foreground_hwnd,
                    relationship: relationship.to_owned(),
                });
            return Ok(postcondition);
        }
        return Err(foreground_lost_delta_error(
            tool,
            source_of_truth,
            timeout_ms,
            &before_hash,
            &after_hash,
            before,
            after,
        ));
    }

    let process_matches = policy
        .expected_process_regex
        .as_ref()
        .is_none_or(|regex| regex.is_match(&after.foreground_process));
    let title_matches = policy
        .expected_title_regex
        .as_ref()
        .is_none_or(|regex| regex.is_match(&after.foreground_title));

    if !process_matches || !title_matches {
        return Err(foreground_change_policy_mismatch_error(
            tool,
            source_of_truth,
            timeout_ms,
            &before_hash,
            &after_hash,
            before,
            after,
            policy,
            process_matches,
            title_matches,
        ));
    }

    tracing::info!(
        code = "ACTION_FOREGROUND_CHANGE_ACCEPTED",
        tool,
        source_of_truth,
        timeout_ms,
        before_hwnd = before.foreground_hwnd,
        after_hwnd = after.foreground_hwnd,
        before_pid = before.foreground_pid,
        after_pid = after.foreground_pid,
        before_process = %before.foreground_process,
        after_process = %after.foreground_process,
        after_title_sha256 = ?after.foreground_title_sha256,
        expected_process_regex = ?policy.expected_process_pattern,
        expected_title_regex = ?policy.expected_title_pattern,
        before_signature = before_hash,
        after_signature = after_hash,
        "verify_delta accepted declared foreground target transition"
    );

    Ok(postcondition_observed_delta(
        tool,
        source_of_truth,
        before_hash,
        after_hash,
        format!(
            "observed expected foreground transition after delivery; before_hwnd=0x{:x}; after_hwnd=0x{:x}; after_process={}; after_title_sha256={}; expected_process_regex_present={}; expected_title_regex_present={}",
            before.foreground_hwnd,
            after.foreground_hwnd,
            after.foreground_process,
            after.foreground_title_sha256.as_deref().unwrap_or("none"),
            policy.expected_process_regex.is_some(),
            policy.expected_title_regex.is_some()
        ),
    ))
}

fn foreground_change_policy_readback(policy: &ForegroundChangePolicy) -> Value {
    json!({
        "allow_foreground_change": policy.allow,
        "expected_foreground_process_regex": policy.expected_process_pattern,
        "expected_foreground_title_regex": policy.expected_title_pattern,
    })
}

fn act_press_policy_params_invalid(
    field: &'static str,
    detail: impl Into<String>,
    reason: &'static str,
) -> ErrorData {
    let detail = detail.into();
    tracing::error!(
        code = error_codes::TOOL_PARAMS_INVALID,
        tool = "act_press",
        field,
        reason,
        detail = %detail,
        "act_press foreground-change policy parameters invalid"
    );
    ErrorData::new(
        ErrorCode(-32099),
        detail.clone(),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "tool": "act_press",
            "field": field,
            "reason": reason,
            "detail": detail,
        })),
    )
}

fn act_press_policy_invalid_regex(
    field: &'static str,
    pattern: &str,
    error: &regex::Error,
) -> ErrorData {
    let detail = format!("{field} is not a valid regex: {error}");
    tracing::error!(
        code = error_codes::TOOL_PARAMS_INVALID,
        tool = "act_press",
        field,
        reason = "invalid_expected_foreground_regex",
        pattern,
        regex_error = %error,
        "act_press foreground-change policy regex invalid"
    );
    ErrorData::new(
        ErrorCode(-32099),
        detail.clone(),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "tool": "act_press",
            "field": field,
            "reason": "invalid_expected_foreground_regex",
            "pattern": pattern,
            "detail": detail,
        })),
    )
}

/// True when any tier of the attempt chain already delivered the click.
fn click_tier_attempts_delivered(tier_attempts: &[ActClickTierAttempt]) -> bool {
    tier_attempts
        .iter()
        .any(|attempt| attempt.status == "delivered")
}

/// #189 R5: only the producer's typed stage marks a returned pattern whose
/// readback failed (delivered, so observation may reconcile it).
fn click_postdispatch_readback_failed(error: &ErrorData) -> bool {
    error
        .data
        .as_ref()
        .is_some_and(|data| data["click_dispatch_stage"] == "readback")
}

/// #218: no-delta readback. `foreground_hwnd` inside the signatures is the observed
/// (target) window, so the real OS foreground is reported next to it, outside the
/// compared signatures (a human switching windows must never count as a delta).
fn click_delta_readback(
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
    os_foreground_hwnd: Option<i64>,
) -> Value {
    json!({
        "before": before,
        "after": after,
        "observed_window_hwnd": after.foreground_hwnd,
        "os_foreground_hwnd": os_foreground_hwnd,
        // #107: true when the only difference is focus, which is not a click effect.
        "focus_only_change": before != after && !click_effect_changed(before, after),
    })
}

fn click_error_data_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn click_tier_attempts_from_error(error: &ErrorData) -> Vec<ActClickTierAttempt> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("tier_attempts"))
        .cloned()
        .and_then(|attempts| serde_json::from_value(attempts).ok())
        .unwrap_or_default()
}

/// The delivered attempt a reconciled post-dispatch error records (#190). It
/// copies the tier, foreground use and `foreground_authorization` of the
/// attempt it reconciles (the first one), so a claimed foreground route is
/// never re-recorded as an unstamped foreground attempt, and a background
/// attempt never inherits another attempt's foreground use.
fn reconciled_click_attempt(
    tier_attempts: &[ActClickTierAttempt],
    error: &ErrorData,
) -> ActClickTierAttempt {
    let reconciled = tier_attempts.first();
    let tier = reconciled.map_or_else(|| "uia".to_owned(), |attempt| attempt.tier.clone());
    let mut attempt = click_tier_delivered(
        tier,
        reconciled.is_some_and(|attempt| attempt.required_foreground),
        format!(
            "delivery reported a post-dispatch UIA readback failure, but verify_delta separately observed target-window SoT mutation; original_error={}",
            error.message
        ),
    );
    attempt.foreground_authorization =
        reconciled.and_then(|attempt| attempt.foreground_authorization.clone());
    attempt
}

fn elements_fingerprint_hash(
    elements: &[AccessibleNode],
    include_focus: bool,
) -> Result<String, ErrorData> {
    let fingerprints: Vec<_> = elements
        .iter()
        .take(160)
        .map(|node| ClickElementFingerprint {
            element_id: node.element_id.to_string(),
            role: node.role.clone(),
            automation_id: node.automation_id.clone(),
            name_sha256: non_empty_sha256(&node.name),
            value_sha256: node.value.as_deref().and_then(non_empty_sha256),
            bbox: node.bbox,
            enabled: node.enabled,
            focused: include_focus && node.focused,
            control_state: node.control_state.clone(),
        })
        .collect();
    hash_json(&fingerprints)
}

fn signature_hash(signature: &ClickDeltaSignature) -> Result<String, ErrorData> {
    hash_json(signature)
}

fn focused_text_candidate(
    focused: Option<&FocusedElement>,
    elements: &[AccessibleNode],
) -> Option<ActTypeFocusedTextCandidate> {
    if let Some(node) = elements
        .iter()
        .find(|node| node.focused && has_text_readback_pattern(&node.patterns))
    {
        return Some(ActTypeFocusedTextCandidate {
            element_id: node.element_id.to_string(),
            role: node.role.clone(),
            name: node.name.clone(),
            selected_text: None,
            bbox: node.bbox,
            value: node.value.clone(),
            patterns: node.patterns.clone(),
        });
    }
    focused.map(|focused| ActTypeFocusedTextCandidate {
        element_id: focused.element_id.to_string(),
        role: focused.role.clone(),
        name: focused.name.clone(),
        selected_text: focused.selected_text.clone(),
        bbox: focused.bbox,
        value: focused.value.clone(),
        patterns: focused.patterns.clone(),
    })
}

fn focused_text_readback(
    focused: Option<&ActTypeFocusedTextCandidate>,
) -> (Option<String>, Option<&'static str>) {
    let Some(focused) = focused else {
        return (None, None);
    };
    if let Some(value) = &focused.value {
        return (Some(value.clone()), Some(ACT_TYPE_TEXT_SOURCE_UIA_VALUE));
    }
    if has_text_readback_pattern(&focused.patterns) {
        return (Some(String::new()), Some(ACT_TYPE_TEXT_SOURCE_UIA_EMPTY));
    }
    (None, None)
}

fn choose_act_type_text_readback(
    focused: Option<&ActTypeFocusedTextCandidate>,
    uia_value: Option<String>,
    uia_readback_source: Option<&'static str>,
    cdp_readback: &CdpActiveTextReadback,
    ocr_readback: &OcrTextReadback,
) -> (Option<String>, Option<String>) {
    if should_prefer_cdp_active_text(
        focused,
        uia_value.as_deref(),
        uia_readback_source,
        cdp_readback,
    ) {
        if let Some(value) = cdp_readback.value.clone() {
            return (
                Some(value),
                Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE.to_owned()),
            );
        }
    }

    let skip_uia = is_browser_shell_uia_readback(focused, uia_value.as_deref());
    if !skip_uia {
        if let Some(value) = uia_value {
            return (Some(value), uia_readback_source.map(str::to_owned));
        }
    }

    if let Some(value) = cdp_readback.value.clone() {
        return (
            Some(value),
            Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE.to_owned()),
        );
    }
    if !skip_uia {
        if let Some(value) = ocr_readback.value.clone() {
            return (
                Some(value),
                Some(ACT_TYPE_TEXT_SOURCE_OCR_FOCUSED_RECT.to_owned()),
            );
        }
    }
    (None, None)
}

/// Normal-bridge tab target of any declared browser row (`browsers/*.json`).
fn is_chrome_bridge_target_id(target_id: &str) -> bool {
    synapse_browsers::BrowserRegistry::global().has_bridge_tab_prefix(target_id)
}

fn chrome_bridge_session_target_parts(target: Option<&SessionTarget>) -> Option<(i64, &str)> {
    let Some(SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id,
    }) = target
    else {
        return None;
    };
    is_chrome_bridge_target_id(cdp_target_id).then_some((*window_hwnd, cdp_target_id.as_str()))
}

fn act_type_chrome_bridge_type_postcondition(
    params: &ActTypeParams,
    emitted: &str,
    chars_typed: u32,
    readback: &crate::chrome_debugger_bridge::ChromeDebuggerTypeActiveElementResult,
) -> Result<ActPostcondition, ErrorData> {
    if readback.chars_typed != chars_typed {
        return Err(postcondition_failed_error(
            "act_type",
            ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_SOURCE_OF_TRUTH,
            format!(
                "Chrome bridge reported chars_typed={} but emitted chars_typed={chars_typed}",
                readback.chars_typed
            ),
            active_element_value_signature(&readback.before_active_element),
            active_element_value_signature(&readback.after_active_element),
            json!({
                "target_id": &readback.target_id,
                "tab_id": readback.tab_id,
                "reported_chars_typed": readback.chars_typed,
                "emitted_chars_typed": chars_typed,
            }),
        ));
    }
    let before_value = readback
        .before_active_element
        .value
        .as_deref()
        .unwrap_or_default();
    let after_value = readback
        .after_active_element
        .value
        .as_deref()
        .unwrap_or_default();
    let expected_value = readback.expected_value.as_deref().unwrap_or(after_value);
    let before_hash = active_element_value_signature(&readback.before_active_element);
    let after_hash = active_element_value_signature(&readback.after_active_element);
    if after_value != expected_value {
        return Err(postcondition_failed_error(
            "act_type",
            ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_SOURCE_OF_TRUTH,
            "Chrome bridge after-read active-element value did not match expected DOM value",
            before_hash,
            after_hash,
            json!({
                "target_id": &readback.target_id,
                "tab_id": readback.tab_id,
                "expected_value_len": expected_value.chars().count(),
                "expected_value_sha256": non_empty_sha256(expected_value),
                "after_value_len": after_value.chars().count(),
                "after_value_sha256": non_empty_sha256(after_value),
                "events_dispatched": &readback.events_dispatched,
                "readback_backend": &readback.readback_backend,
            }),
        ));
    }
    if !params.verify_delta {
        return Ok(postcondition_not_requested(
            "act_type",
            ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_SOURCE_OF_TRUTH,
        ));
    }
    if before_value == after_value {
        return Err(source_no_observed_delta_error(
            "act_type",
            ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_SOURCE_OF_TRUTH,
            params.verify_timeout_ms,
            before_hash,
            after_hash,
            json!({
                "target_id": &readback.target_id,
                "tab_id": readback.tab_id,
                "text_len": emitted.chars().count(),
                "expected_value_len": expected_value.chars().count(),
                "events_dispatched": &readback.events_dispatched,
                "readback_backend": &readback.readback_backend,
            }),
        ));
    }
    Ok(postcondition_observed_delta(
        "act_type",
        ACT_TYPE_CHROME_BRIDGE_ACTIVE_ELEMENT_SOURCE_OF_TRUTH,
        before_hash,
        after_hash,
        format!(
            "observed inactive Chrome bridge active-element value change after background DOM typing; chars_typed={} events={:?}",
            readback.chars_typed, readback.events_dispatched
        ),
    ))
}

fn active_element_value_signature(
    active: &crate::chrome_debugger_bridge::ChromeDebuggerActiveElement,
) -> String {
    let value = active.value.as_deref().unwrap_or_default();
    text_sha256(value)
}

fn chrome_bridge_active_text_readback(
    target_id: &str,
    active: &crate::chrome_debugger_bridge::ChromeDebuggerActiveElement,
) -> CdpActiveTextReadback {
    let value = (active.available).then(|| active.value.clone()).flatten();
    let attempt = if active.available {
        "chrome_bridge_active_element:available".to_owned()
    } else {
        format!(
            "chrome_bridge_active_element:{}",
            active.error_code.as_deref().unwrap_or("unavailable")
        )
    };
    CdpActiveTextReadback {
        value_len: value.as_ref().map(|value| value.chars().count()),
        value_sha256: value.as_deref().and_then(non_empty_sha256),
        value,
        target_id: Some(target_id.to_owned()),
        has_active_element: active.has_active_element,
        is_editable: active.is_editable,
        tag_name: active.tag_name.clone(),
        input_type: None,
        element_path: None,
        id_sha256: active.id.as_deref().and_then(non_empty_sha256),
        name_sha256: active.name.as_deref().and_then(non_empty_sha256),
        error_code: active.error_code.clone(),
        error_detail_sha256: active.error_detail.as_deref().and_then(non_empty_sha256),
        attempt,
    }
}

fn should_prefer_cdp_active_text(
    focused: Option<&ActTypeFocusedTextCandidate>,
    uia_value: Option<&str>,
    uia_readback_source: Option<&'static str>,
    cdp_readback: &CdpActiveTextReadback,
) -> bool {
    if cdp_readback.value.is_none()
        || cdp_readback.has_active_element != Some(true)
        || cdp_readback.is_editable != Some(true)
    {
        return false;
    }
    let tag = cdp_readback.tag_name.as_deref().unwrap_or("").trim();
    if tag.is_empty() || tag.eq_ignore_ascii_case("BODY") || tag.eq_ignore_ascii_case("HTML") {
        return false;
    }
    if focused.is_some_and(|item| item.role.eq_ignore_ascii_case("spin button"))
        && tag.eq_ignore_ascii_case("INPUT")
        && cdp_readback.input_type.as_deref() == Some("date")
        && cdp_readback.element_path.is_some()
    {
        return true;
    }
    if uia_readback_source == Some(ACT_TYPE_TEXT_SOURCE_UIA_EMPTY) {
        return true;
    }
    is_browser_shell_uia_readback(focused, uia_value)
}

fn is_browser_shell_uia_readback(
    focused: Option<&ActTypeFocusedTextCandidate>,
    uia_value: Option<&str>,
) -> bool {
    let Some(focused) = focused else {
        return false;
    };
    if !is_shell_text_focus_role(&focused.role) {
        return false;
    }
    match uia_value.map(str::trim) {
        Some(value) if value.is_empty() => true,
        Some(value) => looks_like_browser_page_value(value),
        None => false,
    }
}

fn is_shell_text_focus_role(role: &str) -> bool {
    matches!(
        role.trim().to_ascii_lowercase().as_str(),
        "window" | "document" | "pane" | "region" | "rootwebarea" | "webarea" | "web view"
    )
}

fn looks_like_browser_page_value(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("data:")
        || value.starts_with("about:")
        || value.starts_with("chrome://")
        || value.starts_with("edge://")
        || value.starts_with("file://")
}

async fn cdp_active_text_readback(input: &ObservationInput) -> CdpActiveTextReadback {
    let Some(cdp) = input.cdp.as_ref() else {
        return CdpActiveTextReadback {
            value: None,
            target_id: None,
            has_active_element: None,
            is_editable: None,
            tag_name: None,
            input_type: None,
            element_path: None,
            id_sha256: None,
            name_sha256: None,
            value_len: None,
            value_sha256: None,
            error_code: None,
            error_detail_sha256: None,
            attempt: "cdp_active_element_value:unavailable:no_cdp_diagnostics".to_owned(),
        };
    };
    let Some(endpoint) = cdp.endpoint.as_deref() else {
        return CdpActiveTextReadback {
            value: None,
            target_id: cdp.selected_target_id.clone(),
            has_active_element: None,
            is_editable: None,
            tag_name: None,
            input_type: None,
            element_path: None,
            id_sha256: None,
            name_sha256: None,
            value_len: None,
            value_sha256: None,
            error_code: cdp.reason_code.clone(),
            error_detail_sha256: cdp.detail.as_deref().and_then(non_empty_sha256),
            attempt: format!(
                "cdp_active_element_value:unavailable:status={}",
                cdp.status.as_str()
            ),
        };
    };
    let Some(target_id) = cdp.selected_target_id.as_deref() else {
        return CdpActiveTextReadback {
            value: None,
            target_id: None,
            has_active_element: None,
            is_editable: None,
            tag_name: None,
            input_type: None,
            element_path: None,
            id_sha256: None,
            name_sha256: None,
            value_len: None,
            value_sha256: None,
            error_code: Some(error_codes::A11Y_CDP_ATTACH_FAILED.to_owned()),
            error_detail_sha256: None,
            attempt: "cdp_active_element_value:unavailable:no_selected_target".to_owned(),
        };
    };

    match synapse_a11y::cdp_active_element_state(endpoint, target_id).await {
        Ok(state) => {
            let value =
                (state.has_active_element && state.is_editable).then(|| state.value.clone());
            CdpActiveTextReadback {
                value,
                target_id: Some(state.target_id),
                has_active_element: Some(state.has_active_element),
                is_editable: Some(state.is_editable),
                tag_name: (!state.tag_name.trim().is_empty()).then_some(state.tag_name),
                input_type: (!state.input_type.is_empty()).then_some(state.input_type),
                element_path: (!state.element_path.is_empty()).then_some(state.element_path),
                id_sha256: non_empty_sha256(&state.id),
                name_sha256: non_empty_sha256(&state.name),
                value_len: Some(state.value.chars().count()),
                value_sha256: Some(text_sha256(&state.value)),
                error_code: None,
                error_detail_sha256: None,
                attempt: if state.has_active_element && state.is_editable {
                    "cdp_active_element_value:available".to_owned()
                } else if state.has_active_element {
                    "cdp_active_element_value:unavailable:active_element_not_editable".to_owned()
                } else {
                    "cdp_active_element_value:unavailable:no_active_element".to_owned()
                },
            }
        }
        Err(error) => CdpActiveTextReadback {
            value: None,
            target_id: Some(target_id.to_owned()),
            has_active_element: None,
            is_editable: None,
            tag_name: None,
            input_type: None,
            element_path: None,
            id_sha256: None,
            name_sha256: None,
            value_len: None,
            value_sha256: None,
            error_code: Some(error.code().to_owned()),
            error_detail_sha256: non_empty_sha256(&error.to_string()),
            attempt: format!("cdp_active_element_value:error:{}", error.code()),
        },
    }
}

fn ocr_focused_rect_text_readback(
    focused: Option<&ActTypeFocusedTextCandidate>,
    elements: &[AccessibleNode],
) -> OcrTextReadback {
    let Some(focused) = focused else {
        return OcrTextReadback {
            value: None,
            word_count: 0,
            value_len: None,
            value_sha256: None,
            attempt: "ocr_focused_rect_text:unavailable:no_focused_element".to_owned(),
        };
    };
    if focused.bbox.w <= 0 || focused.bbox.h <= 0 {
        return OcrTextReadback {
            value: None,
            word_count: 0,
            value_len: None,
            value_sha256: None,
            attempt: "ocr_focused_rect_text:unavailable:empty_focused_bbox".to_owned(),
        };
    }

    let mut words = elements
        .iter()
        .filter(|node| {
            node.automation_id
                .as_deref()
                .is_some_and(|automation_id| automation_id.starts_with("ocr:word:"))
                && rects_intersect(node.bbox, focused.bbox)
                && !node.name.trim().is_empty()
        })
        .collect::<Vec<_>>();
    words.sort_by_key(|node| (node.bbox.y, node.bbox.x));
    let text = words
        .iter()
        .map(|node| node.name.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let word_count = words.len();
    if word_count == 0 {
        return OcrTextReadback {
            value: None,
            word_count,
            value_len: None,
            value_sha256: None,
            attempt: "ocr_focused_rect_text:unavailable:no_ocr_words_in_focused_bbox".to_owned(),
        };
    }
    OcrTextReadback {
        value: Some(text.clone()),
        word_count,
        value_len: Some(text.chars().count()),
        value_sha256: Some(text_sha256(&text)),
        attempt: "ocr_focused_rect_text:available".to_owned(),
    }
}

fn rect_contains_point(rect: Rect, point: Point) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    point.x >= rect.x
        && point.x < rect.x.saturating_add(rect.w)
        && point.y >= rect.y
        && point.y < rect.y.saturating_add(rect.h)
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    if a.w <= 0 || a.h <= 0 || b.w <= 0 || b.h <= 0 {
        return false;
    }
    let a_right = a.x.saturating_add(a.w);
    let a_bottom = a.y.saturating_add(a.h);
    let b_right = b.x.saturating_add(b.w);
    let b_bottom = b.y.saturating_add(b.h);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

fn act_type_foreground_fallback_focus_matches_target(
    target: &ActTypeForegroundFallbackTarget,
    after_click: &ActTypeTextSignature,
) -> Result<(), ErrorData> {
    if after_click.foreground_hwnd != target.root_hwnd {
        return Err(act_type_foreground_fallback_focus_error(
            target,
            after_click,
            "foreground_hwnd_changed_after_target_click",
        ));
    }
    let role = after_click.focused_role.as_deref().unwrap_or_default();
    if !act_type_editable_role(role) {
        return Err(act_type_foreground_fallback_focus_error(
            target,
            after_click,
            "focused_role_is_not_text_editable",
        ));
    }
    if after_click
        .focused_element_id
        .as_deref()
        .is_some_and(|focused_id| focused_id == target.element_id)
    {
        return Ok(());
    }
    if after_click
        .focused_bbox
        .is_some_and(|bbox| rects_intersect(bbox, target.bbox))
    {
        return Ok(());
    }
    Err(act_type_foreground_fallback_focus_error(
        target,
        after_click,
        "focused_element_did_not_match_target_or_bbox",
    ))
}

fn act_type_target_center_point(
    target: &ActTypeForegroundFallbackTarget,
) -> Result<Point, ErrorData> {
    if target.bbox.w <= 0 || target.bbox.h <= 0 {
        return Err(act_type_foreground_fallback_target_invalid_error(
            target,
            "target bbox is empty or inverted",
        ));
    }
    let x = i64::from(target.bbox.x) + i64::from(target.bbox.w) / 2;
    let y = i64::from(target.bbox.y) + i64::from(target.bbox.h) / 2;
    let x = i32::try_from(x).map_err(|err| {
        act_type_foreground_fallback_target_invalid_error(
            target,
            format!("target bbox center x overflowed i32: {err}"),
        )
    })?;
    let y = i32::try_from(y).map_err(|err| {
        act_type_foreground_fallback_target_invalid_error(
            target,
            format!("target bbox center y overflowed i32: {err}"),
        )
    })?;
    Ok(Point { x, y })
}

fn has_text_readback_pattern(patterns: &[UiaPattern]) -> bool {
    patterns
        .iter()
        .any(|pattern| matches!(pattern, UiaPattern::Value | UiaPattern::Text))
}

fn act_type_exact_text_delta(before: &ActTypeTextReadback, value: &str, emitted: &str) -> bool {
    if emitted.is_empty() {
        return false;
    }
    let value = normalize_newlines(value);
    let emitted = normalize_newlines(emitted);
    let Some(previous) = before.value.as_deref() else {
        return value == emitted;
    };
    let previous = normalize_newlines(previous);
    // Preserve existing content around exactly one insertion or the selected
    // replacement. Merely finding the payload cannot prove delivery (#281).
    value.match_indices(&emitted).any(|(offset, _)| {
        let prefix = &value[..offset];
        let suffix = &value[offset + emitted.len()..];
        let removed = previous
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix));
        removed.is_some_and(|removed| {
            removed.is_empty()
                || non_empty_sha256(removed).is_some_and(|digest| {
                    before.signature.focused_selected_text_sha256.as_ref() == Some(&digest)
                })
        })
    })
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

fn text_sha256(value: &str) -> String {
    crate::content_digest::keyed_text_digest_hex(value)
}

// Plain SHA-256 on purpose (#174): before/after equality token over click and
// keyboard delta signatures. Their text fields (names, values, titles) already
// hold keyed digests, so this hashes structure (ids, roles, bboxes) only.
fn hash_json<T: Serialize>(value: &T) -> Result<String, ErrorData> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("failed to encode click delta signature: {error}"),
        )
    })?;
    Ok(hex_encode(&Sha256::digest(bytes)))
}

fn non_empty_sha256(value: &str) -> Option<String> {
    // UIA Win32 edits and browser readback use different newline encodings.
    // Canonicalize both selection signatures and removed-text comparisons.
    let normalized = normalize_newlines(value);
    let trimmed = normalized.trim();
    (!trimmed.is_empty()).then(|| crate::content_digest::keyed_text_digest_hex(trimmed))
}

fn foreground_identity_changed(before: &ClickDeltaSignature, after: &ClickDeltaSignature) -> bool {
    before.foreground_hwnd != after.foreground_hwnd
        || before.foreground_pid != after.foreground_pid
        || before.foreground_process != after.foreground_process
}

fn act_type_foreground_identity_changed(
    before: &ActTypeTextSignature,
    after: &ActTypeTextSignature,
) -> bool {
    before.foreground_hwnd != after.foreground_hwnd
        || before.foreground_pid != after.foreground_pid
        || before.foreground_process != after.foreground_process
}

fn act_type_text_terminal_failure(
    before: &ActTypeTextReadback,
    after: &ActTypeTextReadback,
) -> bool {
    act_type_foreground_identity_changed(&before.signature, &after.signature)
        || (act_type_text_target_changed(&before.signature, &after.signature)
            && act_type_date_parent::shared_date_parent(before, after).is_none())
}

fn verify_act_type_browser_url_response(
    mut response: ActTypeResponse,
    before: ActTypeTextReadback,
    after: ActTypeTextReadback,
    before_hash: String,
    after_hash: String,
    verify_timeout_ms: u32,
    policy: &ActTypeBrowserUrlPolicy,
) -> Result<ActTypeResponse, ErrorData> {
    let before_signature_readback = before.signature.clone();
    let after_signature_readback = after.signature.clone();
    let after_url = after.browser_url.as_deref().ok_or_else(|| {
        postcondition_failed_error(
            "act_type",
            ACT_TYPE_BROWSER_URL_SOURCE_OF_TRUTH,
            "expected_browser_url_regex was set but after-read browser target URL was absent",
            before_hash.clone(),
            after_hash.clone(),
            json!({
                "expected_browser_url_regex": &policy.expected_url_pattern,
                "before": before_signature_readback,
                "after": after_signature_readback,
            }),
        )
    })?;
    if !policy.expected_url_regex.is_match(after_url) {
        return Err(postcondition_failed_error(
            "act_type",
            ACT_TYPE_BROWSER_URL_SOURCE_OF_TRUTH,
            "after-read browser target URL did not match expected_browser_url_regex",
            before_hash,
            after_hash,
            json!({
                "expected_browser_url_regex": &policy.expected_url_pattern,
                "after_url_len": after_url.chars().count(),
                "after_url_sha256": non_empty_sha256(after_url),
                "before": before.signature,
                "after": after.signature,
            }),
        ));
    }
    let before_url = before.browser_url.as_deref();
    response.postcondition = if before_url == Some(after_url) {
        ActPostcondition {
            related_window_observed: None,
            status: "verified_state".to_owned(),
            observed_delta: Some(false),
            source_of_truth: Some(ACT_TYPE_BROWSER_URL_SOURCE_OF_TRUTH.to_owned()),
            before_signature: Some(before_hash),
            after_signature: Some(after_hash),
            detail: Some(format!(
                "act_type verify_delta verified after-read browser target URL matched expected_browser_url_regex; no URL delta was observed within {verify_timeout_ms} ms"
            )),
        }
    } else {
        postcondition_observed_delta(
            "act_type",
            ACT_TYPE_BROWSER_URL_SOURCE_OF_TRUTH,
            before_hash,
            after_hash,
            "observed after-read browser target URL matching expected_browser_url_regex after delivery",
        )
    };
    response.target_readback_required = false;
    response.target_text_integrity = ACT_TYPE_BROWSER_URL_TEXT_INTEGRITY.to_owned();
    Ok(response)
}

fn verify_act_type_text_response(
    response: ActTypeResponse,
    before: ActTypeTextReadback,
    after: ActTypeTextReadback,
    before_hash: String,
    after_hash: String,
    verify_timeout_ms: u32,
    emitted: &str,
) -> Result<ActTypeResponse, ErrorData> {
    if act_type_foreground_identity_changed(&before.signature, &after.signature) {
        return Err(act_type_text_foreground_lost_error(
            verify_timeout_ms,
            &before_hash,
            &after_hash,
            &before.signature,
            &after.signature,
        ));
    }
    if act_type_text_target_changed(&before.signature, &after.signature) {
        return verify_act_type_changed_text_target(
            response,
            &before,
            &after,
            before_hash,
            after_hash,
            verify_timeout_ms,
            emitted,
        );
    }
    verify_act_type_text_delta(
        &before,
        &after,
        &before_hash,
        &after_hash,
        verify_timeout_ms,
        emitted,
    )?;
    Ok(act_type_text_delivery_verified(
        response,
        &before.signature,
        &after.signature,
        before_hash,
        after_hash,
    ))
}

fn act_type_text_delivery_verified(
    mut response: ActTypeResponse,
    before: &ActTypeTextSignature,
    after: &ActTypeTextSignature,
    before_hash: String,
    after_hash: String,
) -> ActTypeResponse {
    let source_of_truth = act_type_text_source_of_truth(before, after);
    response.postcondition = postcondition_observed_delta(
        "act_type",
        &source_of_truth,
        before_hash,
        after_hash,
        "observed selected text Source-of-Truth changed by the exact emitted text after delivery",
    );
    response.target_readback_required = false;
    let source = after.readback_source.as_deref().unwrap_or("unknown");
    response.target_text_integrity = format!("{ACT_TYPE_TEXT_INTEGRITY_PREFIX}:{source}");
    response
}

fn verify_act_type_changed_text_target(
    response: ActTypeResponse,
    before: &ActTypeTextReadback,
    after: &ActTypeTextReadback,
    before_hash: String,
    after_hash: String,
    verify_timeout_ms: u32,
    emitted: &str,
) -> Result<ActTypeResponse, ErrorData> {
    if act_type_date_parent::shared_date_parent(before, after).is_some() {
        return act_type_date_parent::verify_act_type_date_parent_response(
            response,
            before,
            after,
            before_hash,
            after_hash,
            verify_timeout_ms,
            emitted,
        );
    }
    Err(postcondition_failed_error(
        "act_type",
        &act_type_text_source_of_truth(&before.signature, &after.signature),
        "focused text target changed before postcondition readback",
        before_hash,
        after_hash,
        json!({ "before": before.signature, "after": after.signature }),
    ))
}

fn verify_act_type_text_delta(
    before: &ActTypeTextReadback,
    after: &ActTypeTextReadback,
    before_hash: &str,
    after_hash: &str,
    verify_timeout_ms: u32,
    emitted: &str,
) -> Result<(), ErrorData> {
    let source_of_truth = act_type_text_source_of_truth(&before.signature, &after.signature);
    if before.value == after.value {
        return Err(source_no_observed_delta_error(
            "act_type",
            &source_of_truth,
            verify_timeout_ms,
            before_hash.to_owned(),
            after_hash.to_owned(),
            json!({
                "before": before.signature,
                "after": after.signature,
            }),
        ));
    }
    let Some(after_value) = after.value.as_deref() else {
        return Err(act_type_verify_surface_unavailable_error(
            "after-read had no UIA, CDP, or OCR text Source-of-Truth surface for act_type verify_delta",
            after_hash.to_owned(),
            after.signature.clone(),
        ));
    };
    if !act_type_exact_text_delta(before, after_value, emitted)
        && !segmented_date_value_matches(&after.signature, after_value, emitted)
    {
        return Err(postcondition_failed_error(
            "act_type",
            &source_of_truth,
            "text Source-of-Truth changed but did not match the exact emitted text delta",
            before_hash.to_owned(),
            after_hash.to_owned(),
            json!({
                "expected_emitted_len": emitted.chars().count(),
                "expected_emitted_sha256": text_sha256(emitted),
                "before": before.signature,
                "after": after.signature,
            }),
        ));
    }
    Ok(())
}

fn act_type_error_allows_visual_delta(error: &ErrorData) -> bool {
    match click_error_data_code(error) {
        Some(error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE) => true,
        Some(error_codes::ACTION_NO_OBSERVED_DELTA) => {
            act_type_no_observed_delta_has_no_text_surface(error)
        }
        _ => false,
    }
}

fn act_type_no_observed_delta_has_no_text_surface(error: &ErrorData) -> bool {
    let readback = error.data.as_ref().and_then(|data| {
        data.pointer("/verify_delta/readback")
            .or_else(|| data.get("readback"))
    });
    let Some(readback) = readback else {
        return false;
    };
    let before_has_text = readback
        .pointer("/before/has_text_readback")
        .and_then(Value::as_bool);
    let after_has_text = readback
        .pointer("/after/has_text_readback")
        .and_then(Value::as_bool);
    before_has_text == Some(false) && after_has_text == Some(false)
}

fn act_type_text_source_of_truth(
    before: &ActTypeTextSignature,
    after: &ActTypeTextSignature,
) -> String {
    after
        .readback_source
        .as_ref()
        .or(before.readback_source.as_ref())
        .map(|source| format!("{ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH}:{source}"))
        .unwrap_or_else(|| ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH.to_owned())
}

fn act_type_text_target_changed(
    before: &ActTypeTextSignature,
    after: &ActTypeTextSignature,
) -> bool {
    if before.focused_element_id != after.focused_element_id
        && !same_segmented_date_input(before, after)
    {
        return true;
    }
    if before.readback_source.as_deref() == Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE)
        || after.readback_source.as_deref() == Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE)
    {
        return before.cdp_selected_target_id != after.cdp_selected_target_id
            || before.cdp_active_element_path != after.cdp_active_element_path
            || before.cdp_active_tag_name != after.cdp_active_tag_name
            || before.cdp_active_id_sha256 != after.cdp_active_id_sha256
            || before.cdp_active_name_sha256 != after.cdp_active_name_sha256;
    }
    if before.readback_source.as_deref() == Some(ACT_TYPE_TEXT_SOURCE_OCR_FOCUSED_RECT)
        || after.readback_source.as_deref() == Some(ACT_TYPE_TEXT_SOURCE_OCR_FOCUSED_RECT)
    {
        return before.focused_bbox != after.focused_bbox;
    }
    false
}

fn same_segmented_date_input(before: &ActTypeTextSignature, after: &ActTypeTextSignature) -> bool {
    before.readback_source.as_deref() == Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE)
        && after.readback_source.as_deref() == Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE)
        && before.cdp_active_input_type.as_deref() == Some("date")
        && after.cdp_active_input_type.as_deref() == Some("date")
        && before.cdp_selected_target_id == after.cdp_selected_target_id
        && before.cdp_active_element_path.is_some()
        && before.cdp_active_element_path == after.cdp_active_element_path
}

fn segmented_date_value_matches(
    signature: &ActTypeTextSignature,
    value: &str,
    emitted: &str,
) -> bool {
    if signature.cdp_active_input_type.as_deref() != Some("date")
        || signature.readback_source.as_deref() != Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE)
        || emitted.len() != 8
        || !emitted.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4] == emitted.as_bytes()[4..]
        && bytes[5..7] == emitted.as_bytes()[..2]
        && bytes[8..10] == emitted.as_bytes()[2..4]
}

/// Structured failure for the `act_set_field_text` foreground tier: precise
/// reason code, full target evidence, and the action error detail.
fn set_field_text_foreground_error(
    target: &ActTypeForegroundFallbackTarget,
    code: &'static str,
    reason: &'static str,
    detail: impl Into<String>,
) -> ErrorData {
    let detail = detail.into();
    tracing::error!(
        code,
        tool = "act_set_field_text",
        element_id = %target.element_id,
        root_hwnd = target.root_hwnd,
        reason,
        detail = %detail,
        "act_set_field_text foreground tier failed"
    );
    ErrorData::new(
        ErrorCode(-32099),
        detail.clone(),
        Some(json!({
            "code": code,
            "tool": "act_set_field_text",
            "reason": reason,
            "detail": detail,
            "target": target,
            "backend_tier_used": crate::m2::TIER_FOREGROUND_KEYS,
            "required_foreground": true,
        })),
    )
}

/// The window the preflight verified as foreground (after any focus step):
/// the target that foreground typing must keep protecting (#188).
pub(super) fn preflight_foreground_hwnd(preflight: &ActionPreflightReadback) -> i64 {
    preflight.after.as_ref().unwrap_or(&preflight.before).hwnd
}

/// Password targets never expose value content; verification compares the
/// UTF-16 password length Source of Truth instead, mirroring `act_set_value`.
fn set_field_text_password_response(
    params: &crate::m2::ActSetFieldTextParams,
    started: Instant,
    method: &str,
    before: &synapse_a11y::ElementValueReadback,
    after: &synapse_a11y::ElementValueReadback,
) -> Result<crate::m2::ActSetFieldTextResponse, ErrorData> {
    let element_id = crate::m2::required_element_id(params)?;
    let expected_len = params.text.encode_utf16().count();
    let before_len = before.password_len.unwrap_or(0);
    let after_len = after.password_len.unwrap_or(0);
    let signature = |len: usize| format!("password_len:{len}");
    let requested_len = u32::try_from(params.text.chars().count()).unwrap_or(u32::MAX);
    if after.password_len != Some(expected_len) {
        tracing::error!(
            code = error_codes::ACTION_POSTCONDITION_FAILED,
            tool = "act_set_field_text",
            element_id = %element_id,
            method,
            before_len,
            after_len,
            expected_len,
            "act_set_field_text password-length readback did not equal requested length"
        );
        return Err(ErrorData::new(
            ErrorCode(-32099),
            "act_set_field_text Source-of-Truth postcondition failed: password target length readback does not equal requested text length",
            Some(json!({
                "code": error_codes::ACTION_POSTCONDITION_FAILED,
                "tool": "act_set_field_text",
                "method": method,
                "source_of_truth": crate::m2::SOURCE_UIA_PASSWORD_LENGTH,
                "before_len": before_len,
                "after_len": after_len,
                "expected_len": expected_len,
                "is_password": true,
            })),
        ));
    }
    let changed = before.password_len != after.password_len;
    Ok(crate::m2::ActSetFieldTextResponse {
        ok: true,
        method: method.to_owned(),
        backend_tier_used: crate::m2::TIER_FOREGROUND_KEYS.to_owned(),
        required_foreground: true,
        source_of_truth: crate::m2::SOURCE_UIA_PASSWORD_LENGTH.to_owned(),
        requested_len,
        before_len: u32::try_from(before_len).unwrap_or(u32::MAX),
        after_len: u32::try_from(after_len).unwrap_or(u32::MAX),
        requested_sha256: signature(expected_len),
        before_sha256: signature(before_len),
        after_sha256: signature(after_len),
        changed,
        postcondition: ActPostcondition {
            related_window_observed: None,
            status: "verified_state".to_owned(),
            observed_delta: Some(changed),
            source_of_truth: Some(crate::m2::SOURCE_UIA_PASSWORD_LENGTH.to_owned()),
            before_signature: Some(signature(before_len)),
            after_signature: Some(signature(after_len)),
            detail: Some(
                "act_set_field_text password target length equals requested length; value content intentionally not read or compared"
                    .to_owned(),
            ),
        },
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}

fn act_type_foreground_fallback_recording_error(
    target: &ActTypeForegroundFallbackTarget,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        "act_type into_element Chromium foreground fallback requires the live foreground input tier and cannot run against the recording backend",
        Some(json!({
            "code": error_codes::ACTION_BACKEND_UNAVAILABLE,
            "tool": "act_type",
            "reason": "chromium_foreground_fallback_recording_backend_unsupported",
            "target": target,
            "required_foreground": true,
            "target_readback_required": true,
        })),
    )
}

fn act_type_foreground_fallback_target_not_foreground_error(
    expected: &ForegroundProof,
    target: &ActTypeForegroundFallbackTarget,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "act_type into_element Chromium foreground fallback requires target hwnd 0x{:x} to be the current foreground hwnd, but preflight foreground was 0x{:x}",
            target.root_hwnd, expected.hwnd
        ),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_LOST,
            "tool": "act_type",
            "reason": "into_element_foreground_fallback_target_not_foreground",
            "foreground_expected": expected,
            "target": target,
            "required_foreground": true,
            "target_readback_required": true,
        })),
    )
}

fn act_type_foreground_fallback_target_invalid_error(
    target: &ActTypeForegroundFallbackTarget,
    detail: impl Into<String>,
) -> ErrorData {
    let detail = detail.into();
    ErrorData::new(
        ErrorCode(-32099),
        format!("act_type into_element Chromium foreground fallback target invalid: {detail}"),
        Some(json!({
            "code": error_codes::ACTION_TARGET_INVALID,
            "tool": "act_type",
            "reason": "chromium_foreground_fallback_target_invalid",
            "detail": detail,
            "target": target,
            "required_foreground": true,
            "target_readback_required": true,
        })),
    )
}

fn act_type_foreground_fallback_actionability_error(
    target: &ActTypeForegroundFallbackTarget,
    refusal: &foreground_click_actionability::ClickPointRefusal,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "act_type into_element Chromium foreground fallback refused to click: {}",
            refusal.detail
        ),
        Some(json!({
            "code": refusal.code,
            "tool": "act_type",
            "reason": refusal.reason,
            "detail": refusal.detail,
            "target": target,
            "required_foreground": true,
            "target_readback_required": true,
        })),
    )
}

fn act_type_foreground_fallback_click_error(
    target: &ActTypeForegroundFallbackTarget,
    point: Point,
    error: &ActionError,
) -> ErrorData {
    let mapped = crate::m2::action_error_to_mcp(error);
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "act_type into_element Chromium foreground fallback click failed at ({},{}): {}",
            point.x, point.y, error
        ),
        Some(json!({
            "code": mapped
                .data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(Value::as_str)
                .unwrap_or(error_codes::ACTION_BACKEND_UNAVAILABLE),
            "tool": "act_type",
            "reason": "chromium_foreground_fallback_click_failed",
            "point": point,
            "target": target,
            "cause": mapped.data,
            "required_foreground": true,
            "target_readback_required": true,
        })),
    )
}

fn act_type_foreground_fallback_focus_error(
    target: &ActTypeForegroundFallbackTarget,
    after_click: &ActTypeTextSignature,
    reason: &'static str,
) -> ErrorData {
    let after_hash =
        verify_hash_json(after_click).unwrap_or_else(|_| "hash_unavailable".to_owned());
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "act_type into_element Chromium foreground fallback click did not focus the requested editable target: {reason}"
        ),
        Some(json!({
            "code": error_codes::ACTION_TARGET_INVALID,
            "tool": "act_type",
            "reason": reason,
            "source_of_truth": ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH,
            "after_click_signature": after_hash,
            "after_click": after_click,
            "target": target,
            "required_foreground": true,
            "target_readback_required": true,
        })),
    )
}

fn act_type_verify_surface_unavailable_error(
    detail: impl Into<String>,
    signature_hash: String,
    readback: ActTypeTextSignature,
) -> ErrorData {
    let detail = detail.into();
    tracing::error!(
        code = error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE,
        tool = "act_type",
        source_of_truth = ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH,
        signature = %signature_hash,
        readback_source = ?readback.readback_source,
        attempts = ?readback.text_readback_attempts,
        "act_type verify_delta text Source-of-Truth surface unavailable"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!("act_type verify_delta Source-of-Truth surface unavailable: {detail}"),
        Some(json!({
            "code": error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE,
            "tool": "act_type",
            "source_of_truth": ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH,
            "detail": detail,
            "signature": signature_hash,
            "readback": readback,
        })),
    )
}

fn act_type_browser_url_readback_error(
    code: &str,
    detail: impl Into<String>,
    cdp_status: Option<&str>,
    endpoint: Option<&str>,
    cdp_detail: Option<&str>,
) -> ErrorData {
    let detail = detail.into();
    tracing::error!(
        code,
        tool = "act_type",
        source_of_truth = ACT_TYPE_BROWSER_URL_SOURCE_OF_TRUTH,
        cdp_status,
        endpoint,
        detail = %detail,
        cdp_detail,
        "act_type browser URL Source-of-Truth readback failed"
    );
    ErrorData::new(
        ErrorCode(-32099),
        detail.clone(),
        Some(json!({
            "code": code,
            "tool": "act_type",
            "source_of_truth": ACT_TYPE_BROWSER_URL_SOURCE_OF_TRUTH,
            "cdp_status": cdp_status,
            "endpoint": endpoint,
            "cdp_detail": cdp_detail,
            "detail": detail,
        })),
    )
}

fn act_type_text_foreground_lost_error(
    timeout_ms: u32,
    before_hash: &str,
    after_hash: &str,
    before: &ActTypeTextSignature,
    after: &ActTypeTextSignature,
) -> ErrorData {
    tracing::warn!(
        code = error_codes::ACTION_FOREGROUND_LOST,
        tool = "act_type",
        source_of_truth = ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH,
        timeout_ms,
        before_hwnd = before.foreground_hwnd,
        after_hwnd = after.foreground_hwnd,
        before_pid = before.foreground_pid,
        after_pid = after.foreground_pid,
        before_process = %before.foreground_process,
        after_process = %after.foreground_process,
        before_signature = before_hash,
        after_signature = after_hash,
        "act_type text readback foreground target identity changed before postcondition readback"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "act_type text readback cannot accept observed delta because foreground target changed within {timeout_ms} ms"
        ),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_LOST,
            "tool": "act_type",
            "source_of_truth": ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH,
            "verify_delta": {
                "timeout_ms": timeout_ms,
                "before_signature": before_hash,
                "after_signature": after_hash,
                "before": before,
                "after": after,
            }
        })),
    )
}

fn foreground_lost_delta_error(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before_hash: &str,
    after_hash: &str,
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
) -> ErrorData {
    tracing::warn!(
        code = error_codes::ACTION_FOREGROUND_LOST,
        tool,
        source_of_truth,
        timeout_ms,
        before_hwnd = before.foreground_hwnd,
        after_hwnd = after.foreground_hwnd,
        before_pid = before.foreground_pid,
        after_pid = after.foreground_pid,
        before_process = %before.foreground_process,
        after_process = %after.foreground_process,
        before_title_sha256 = ?before.foreground_title_sha256,
        after_title_sha256 = ?after.foreground_title_sha256,
        before_signature = before_hash,
        after_signature = after_hash,
        "verify_delta foreground target identity changed before postcondition readback"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} verify_delta cannot accept observed delta because foreground target changed within {timeout_ms} ms"
        ),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_LOST,
            "reason": "unexpected_foreground_change",
            "tool": tool,
            "source_of_truth": source_of_truth,
            "verify_delta": {
                "timeout_ms": timeout_ms,
                "before_signature": before_hash,
                "after_signature": after_hash,
                "before": before,
                "after": after,
            }
        })),
    )
}

fn foreground_change_policy_mismatch_error(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before_hash: &str,
    after_hash: &str,
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
    policy: &ForegroundChangePolicy,
    process_matches: bool,
    title_matches: bool,
) -> ErrorData {
    tracing::error!(
        code = error_codes::ACTION_FOREGROUND_LOST,
        reason = "foreground_change_policy_mismatch",
        tool,
        source_of_truth,
        timeout_ms,
        before_hwnd = before.foreground_hwnd,
        after_hwnd = after.foreground_hwnd,
        before_pid = before.foreground_pid,
        after_pid = after.foreground_pid,
        before_process = %before.foreground_process,
        after_process = %after.foreground_process,
        before_title_sha256 = ?before.foreground_title_sha256,
        after_title_sha256 = ?after.foreground_title_sha256,
        expected_process_regex = ?policy.expected_process_pattern,
        expected_title_regex = ?policy.expected_title_pattern,
        process_matches,
        title_matches,
        before_signature = before_hash,
        after_signature = after_hash,
        "verify_delta foreground target changed but did not match declared policy"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} verify_delta foreground target changed but did not match expected foreground policy within {timeout_ms} ms"
        ),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_LOST,
            "reason": "foreground_change_policy_mismatch",
            "tool": tool,
            "source_of_truth": source_of_truth,
            "foreground_change_policy": foreground_change_policy_readback(policy),
            "matches": {
                "process": process_matches,
                "title": title_matches,
            },
            "verify_delta": {
                "timeout_ms": timeout_ms,
                "before_signature": before_hash,
                "after_signature": after_hash,
                "before": before,
                "after": after,
            }
        })),
    )
}

fn act_stroke_audit_details(stroke_details: &Value, preflight: &ActionPreflightReadback) -> Value {
    json!({
        "stroke": stroke_details,
        "preflight": preflight,
    })
}

fn act_stroke_failure_audit_details(
    stroke_details: &Value,
    preflight: &ActionPreflightReadback,
    error: &ErrorData,
) -> Value {
    json!({
        "stroke": stroke_details,
        "preflight": preflight,
        "failure": act_stroke_error_details(error),
    })
}

fn log_act_stroke_failure(details: &Value, error: &ErrorData) {
    let stroke = details.get("stroke").unwrap_or(&Value::Null);
    let failure = details.get("failure").unwrap_or(&Value::Null);
    let preflight = details.get("preflight").unwrap_or(&Value::Null);
    let error_code = failure
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    let path_id = stroke
        .get("path_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let path_kind = stroke
        .get("path_kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let backend_requested = stroke
        .get("backend_requested")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let backend_resolved = stroke
        .get("backend_resolved")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let point_index = failure.get("point_index").cloned().unwrap_or(Value::Null);
    let queue_rate_state = failure
        .get("queue_rate_state")
        .cloned()
        .unwrap_or_else(|| json!({ "kind": "not_rate_or_queue" }));
    let foreground_proof = preflight
        .get("after")
        .or_else(|| preflight.get("before"))
        .cloned()
        .unwrap_or(Value::Null);

    tracing::error!(
        code = error_code,
        detail = %error.message,
        path_id,
        path_kind,
        backend_requested,
        backend_resolved,
        point_index = ?point_index,
        queue_rate_state = ?queue_rate_state,
        foreground_proof = ?foreground_proof,
        fallback_path_executed = false,
        action_kind = "act_stroke",
        "act_stroke failed without fallback"
    );
}

fn require_diagnostic_confirm(
    actual: &str,
    expected: &'static str,
    tool: &'static str,
) -> Result<(), ErrorData> {
    if actual == expected {
        return Ok(());
    }
    Err(ErrorData::new(
        ErrorCode(-32099),
        format!("{tool} requires confirm=\"{expected}\""),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "detail_code": "ACTION_DIAGNOSTIC_CONFIRM_MISMATCH",
            "expected_confirm": expected,
            "actual_confirm_present": !actual.is_empty(),
        })),
    ))
}

fn validate_diagnostic_ttl_ms(ttl_ms: u64) -> Result<(), ErrorData> {
    if (ACTION_DIAGNOSTIC_MIN_TTL_MS..=ACTION_DIAGNOSTIC_MAX_TTL_MS).contains(&ttl_ms) {
        return Ok(());
    }
    Err(ErrorData::new(
        ErrorCode(-32099),
        format!(
            "diagnostic ttl_ms must be between {ACTION_DIAGNOSTIC_MIN_TTL_MS} and {ACTION_DIAGNOSTIC_MAX_TTL_MS}; got {ttl_ms}"
        ),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "detail_code": "ACTION_DIAGNOSTIC_TTL_OUT_OF_RANGE",
            "min_ttl_ms": ACTION_DIAGNOSTIC_MIN_TTL_MS,
            "max_ttl_ms": ACTION_DIAGNOSTIC_MAX_TTL_MS,
            "ttl_ms": ttl_ms,
        })),
    ))
}

fn validate_queue_blocker_duration_ms(duration_ms: u32) -> Result<(), ErrorData> {
    if (ACTION_DIAGNOSTIC_MIN_QUEUE_BLOCKER_MS..=ACTION_DIAGNOSTIC_MAX_QUEUE_BLOCKER_MS)
        .contains(&duration_ms)
    {
        return Ok(());
    }
    Err(ErrorData::new(
        ErrorCode(-32099),
        format!(
            "diagnostic blocker_duration_ms must be between {ACTION_DIAGNOSTIC_MIN_QUEUE_BLOCKER_MS} and {ACTION_DIAGNOSTIC_MAX_QUEUE_BLOCKER_MS}; got {duration_ms}"
        ),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "detail_code": "ACTION_DIAGNOSTIC_QUEUE_BLOCKER_DURATION_OUT_OF_RANGE",
            "min_blocker_duration_ms": ACTION_DIAGNOSTIC_MIN_QUEUE_BLOCKER_MS,
            "max_blocker_duration_ms": ACTION_DIAGNOSTIC_MAX_QUEUE_BLOCKER_MS,
            "blocker_duration_ms": duration_ms,
        })),
    ))
}

fn token_bucket_readback(snapshot: TokenBucketSnapshot) -> TokenBucketReadback {
    TokenBucketReadback {
        capacity: snapshot.capacity,
        tokens: snapshot.tokens,
        refill_rate_per_s: snapshot.refill_rate_per_s,
        last_refill_ns: snapshot.last_refill_ns,
    }
}

fn diagnostic_adjacent_point(from: Point) -> Point {
    let x = if from.x == i32::MAX {
        from.x - 1
    } else {
        from.x + 1
    };
    Point { x, y: from.y }
}

fn diagnostic_queue_blocker_action(from: Point, to: Point, duration_ms: u32) -> Action {
    Action::MouseStroke {
        path: PathSpec::Line {
            from: PathPoint::from(from),
            to: PathPoint::from(to),
        },
        button: None,
        profile: VelocityProfile::Constant,
        timing: StrokeTiming::DurationMs { duration_ms },
        motion_model: StrokeMotionModel::Path,
        humanize: None,
        backend: Backend::Software,
    }
}

fn saturate_action_queue(
    handle: &synapse_action::ActionHandle,
) -> Result<(u32, u32, bool), ErrorData> {
    let mut filler_attempts = 0_u32;
    let mut queued_fillers = 0_u32;
    for _ in 0..=ACTION_QUEUE_CAPACITY {
        filler_attempts = filler_attempts.saturating_add(1);
        let action = Action::MouseMoveRelative {
            dx: 0.0,
            dy: 0.0,
            backend: Backend::Software,
        };
        match handle.try_execute(action) {
            Ok(()) => {
                queued_fillers = queued_fillers.saturating_add(1);
            }
            Err(ActionError::QueueFull { .. }) => {
                return Ok((filler_attempts, queued_fillers, true));
            }
            Err(error) => {
                return Err(diagnostic_action_error_to_mcp(error));
            }
        }
    }
    Ok((filler_attempts, queued_fillers, false))
}

fn diagnostic_action_error_to_mcp(error: ActionError) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        error.to_string(),
        Some(json!({
            "code": error.code(),
            "detail": error.detail(),
            "retry_after_ms": error.retry_after_ms(),
            "queue_rate_state": diagnostic_queue_rate_state(&error),
        })),
    )
}

fn diagnostic_queue_rate_state(error: &ActionError) -> Value {
    match error {
        ActionError::RateLimited {
            retry_after_ms,
            detail,
        } => json!({
            "kind": "rate_limited",
            "retry_after_ms": retry_after_ms,
            "detail": detail,
        }),
        ActionError::QueueFull { detail } => json!({
            "kind": "queue_full",
            "detail": detail,
        }),
        _ => json!({
            "kind": "not_rate_or_queue",
        }),
    }
}

impl SynapseService {
    fn start_act_stroke_foreground_monitor(
        &self,
        preflight: &ActionPreflightReadback,
    ) -> ActStrokeForegroundMonitor {
        let cancel = CancellationToken::new();
        let expected = preflight
            .after
            .clone()
            .unwrap_or_else(|| preflight.before.clone());
        let task = tokio::spawn(monitor_act_stroke_foreground(
            self.clone(),
            expected,
            cancel.clone(),
        ));
        ActStrokeForegroundMonitor { cancel, task }
    }
}

struct ActStrokeForegroundMonitor {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Option<ErrorData>>,
}

async fn await_act_stroke_foreground_monitor(
    monitor: Option<ActStrokeForegroundMonitor>,
) -> Option<ErrorData> {
    let Some(monitor) = monitor else {
        return None;
    };
    monitor.cancel.cancel();
    match monitor.task.await {
        Ok(error) => error,
        Err(error) => Some(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("act_stroke foreground monitor join failed: {error}"),
        )),
    }
}

const fn should_monitor_act_stroke_foreground(
    recording_active: bool,
    requires_input_lease: bool,
) -> bool {
    !recording_active && requires_input_lease
}

const fn should_acquire_act_stroke_input_lease(
    recording_active: bool,
    requires_input_lease: bool,
) -> bool {
    !recording_active && requires_input_lease
}

async fn monitor_act_stroke_foreground(
    service: SynapseService,
    expected: ForegroundProof,
    cancel: CancellationToken,
) -> Option<ErrorData> {
    loop {
        tokio::select! {
            () = cancel.cancelled() => return None,
            () = tokio::time::sleep(std::time::Duration::from_millis(ACT_STROKE_FOREGROUND_MONITOR_INTERVAL_MS)) => {}
        }

        match service.current_audit_foreground() {
            Ok(actual) if actual.hwnd == expected.hwnd => {}
            Ok(actual) => {
                synapse_action::request_release_interrupt();
                tracing::error!(
                    code = error_codes::ACTION_FOREGROUND_LOST,
                    expected_hwnd = expected.hwnd,
                    actual_hwnd = actual.hwnd,
                    expected_pid = expected.pid,
                    actual_pid = actual.pid,
                    expected_process_name = %expected.process_name,
                    actual_process_name = %actual.process_name,
                    expected_window_title = %expected.window_title,
                    actual_window_title = %actual.window_title,
                    action_kind = "act_stroke",
                    "act_stroke foreground lost mid-stroke; release interrupt requested"
                );
                return Some(act_stroke_foreground_lost_error(
                    &expected,
                    Some(&actual),
                    None,
                ));
            }
            Err(error) => {
                synapse_action::request_release_interrupt();
                tracing::error!(
                    code = error_codes::ACTION_FOREGROUND_LOST,
                    expected_hwnd = expected.hwnd,
                    expected_pid = expected.pid,
                    expected_process_name = %expected.process_name,
                    expected_window_title = %expected.window_title,
                    read_error = %error.message,
                    action_kind = "act_stroke",
                    "act_stroke foreground read failed mid-stroke; release interrupt requested"
                );
                return Some(act_stroke_foreground_lost_error(
                    &expected,
                    None,
                    Some(&error),
                ));
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct BackgroundTargetForegroundGuard {
    pub(super) element_hwnd: i64,
    pub(super) root_hwnd: i64,
}

struct BackgroundForegroundSection {
    target: BackgroundTargetForegroundGuard,
    _section: tokio::sync::MutexGuard<'static, ()>,
}

impl BackgroundForegroundSection {
    async fn enter(target: Option<BackgroundTargetForegroundGuard>) -> Option<Self> {
        let target = target?;
        Some(Self {
            target,
            _section: native_foreground_section().await,
        })
    }
}

fn act_set_value_target_foreground_guard(
    element_id: &ElementId,
) -> Result<BackgroundTargetForegroundGuard, ErrorData> {
    let hwnd = element_id.parts().map_err(|error| {
        mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "act_set_value element id {element_id} could not be parsed for foreground guard: {error}"
            ),
        )
    })?.hwnd;
    let root_hwnd = crate::m2::live_target_root(
        hwnd,
        &format!("act_set_value element id {element_id} target window check"),
    )?;
    Ok(BackgroundTargetForegroundGuard {
        element_hwnd: hwnd,
        root_hwnd,
    })
}

fn set_field_text_locator_window_hwnd(
    params: &crate::m2::ActSetFieldTextParams,
    locator: &crate::m2::ActSetFieldTextLocator,
    target: Option<&SessionTarget>,
) -> Result<i64, ErrorData> {
    if let Some(window_hwnd) = locator.window_hwnd {
        return Ok(window_hwnd);
    }
    if let Some(target) = target {
        return Ok(match target {
            SessionTarget::Window { hwnd } => *hwnd,
            SessionTarget::Cdp { window_hwnd, .. } => *window_hwnd,
        });
    }
    if let Some(element_id) = params.element_id.as_ref() {
        let hwnd = element_id
            .parts()
            .map_err(|error| {
                mcp_error(
                    error_codes::ACTION_TARGET_INVALID,
                    format!(
                        "act_set_field_text locator could not use element_id HWND hint from {element_id}: {error}"
                    ),
                )
            })?
            .hwnd;
        return synapse_a11y::top_level_root_hwnd(hwnd).or(Ok(hwnd));
    }
    Err(mcp_error(
        error_codes::TARGET_NOT_SET,
        "act_set_field_text locator requires locator.window_hwnd, an active session target, or an element_id HWND hint; refusing to use the human foreground implicitly",
    ))
}

fn set_field_text_locator_find_params(
    locator: &crate::m2::ActSetFieldTextLocator,
    window_hwnd: i64,
) -> FindParams {
    FindParams {
        query: None,
        role: trimmed_non_empty(locator.role.as_deref()),
        name_substring: trimmed_non_empty(
            locator
                .name_substring
                .as_deref()
                .or(locator.name.as_deref()),
        ),
        automation_id: trimmed_non_empty(locator.automation_id.as_deref()),
        scope: Some(FindScope::Elements),
        limit: Some(20),
        in_window: None,
        window_hwnd: Some(window_hwnd),
    }
}

fn set_field_text_find_result_is_element(result: &FindResult) -> bool {
    result.kind == FindResultKind::Element && result.element_id.is_some()
}

fn set_field_text_locator_exact_name_matches(
    locator: &crate::m2::ActSetFieldTextLocator,
    result: &FindResult,
) -> bool {
    let Some(expected) = locator.name.as_deref().map(str::trim) else {
        return true;
    };
    result
        .name
        .as_deref()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
}

fn set_field_text_locator_resolution_error(
    tool: &'static str,
    locator: &crate::m2::ActSetFieldTextLocator,
    window_hwnd: i64,
    resolution_phase: &'static str,
    reason: &'static str,
    candidates: &[FindResult],
) -> ErrorData {
    let code = if reason == "ambiguous" {
        error_codes::ACTION_TARGET_INVALID
    } else {
        error_codes::ACTION_ELEMENT_NOT_RESOLVED
    };
    let detail = if reason == "ambiguous" {
        let roles = candidates
            .iter()
            .take(5)
            .map(|candidate| candidate.role.as_deref().unwrap_or("unknown"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{tool} locator matched {} elements in window 0x{window_hwnd:x} (roles: {roles}); refine role/name/automation_id so exactly one element resolves",
            candidates.len()
        )
    } else {
        format!("{tool} locator did not match any element in window 0x{window_hwnd:x}")
    };
    ErrorData::new(
        ErrorCode(-32099),
        detail,
        Some(json!({
            "code": code,
            "tool": tool,
            "reason": reason,
            "resolution_phase": resolution_phase,
            "window_hwnd": window_hwnd,
            "locator": set_field_text_locator_error_details(locator),
            "candidate_count": candidates.len(),
            "candidates": candidates
                .iter()
                .take(5)
                .map(set_field_text_locator_candidate_details)
                .collect::<Vec<_>>(),
        })),
    )
}

fn set_field_text_locator_error_details(locator: &crate::m2::ActSetFieldTextLocator) -> Value {
    json!({
        "window_hwnd": locator.window_hwnd,
        "role": locator.role.as_deref(),
        "name_sha256": locator
            .name
            .as_deref()
            .map(crate::m2::postcondition::text_signature),
        "name_len": locator.name.as_ref().map(|name| name.chars().count()),
        "name_substring_sha256": locator
            .name_substring
            .as_deref()
            .map(crate::m2::postcondition::text_signature),
        "name_substring_len": locator
            .name_substring
            .as_ref()
            .map(|name| name.chars().count()),
        "automation_id": locator.automation_id.as_deref(),
    })
}

fn set_field_text_locator_candidate_details(result: &FindResult) -> Value {
    json!({
        "element_id": result.element_id.as_ref().map(ToString::to_string),
        "role": result.role.as_deref(),
        "name_sha256": result
            .name
            .as_deref()
            .map(crate::m2::postcondition::text_signature),
        "name_len": result.name.as_ref().map(|name| name.chars().count()),
        "automation_id": result.automation_id.as_deref(),
        "bbox": result.bbox,
        "score": result.score,
    })
}

fn set_field_text_error_is_stale<T>(result: &Result<T, ErrorData>) -> bool {
    matches!(
        result,
        Err(error) if mcp_error_data_code(error) == Some(error_codes::A11Y_ELEMENT_STALE)
    )
}

fn mcp_error_data_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn act_click_target_foreground_guard(
    params: &ActClickParams,
) -> Result<Option<BackgroundTargetForegroundGuard>, ErrorData> {
    // #153: CDP web-node clicks never move the OS foreground; nothing to guard.
    if click_auto_wait_element_id(params)
        .is_some_and(|id| synapse_a11y::cdp_backend_from_element_id(id).is_some())
    {
        return Ok(None);
    }
    Ok(
        click_target_foreground_guard_hwnds(params)?.map(|(element_hwnd, root_hwnd)| {
            BackgroundTargetForegroundGuard {
                element_hwnd,
                root_hwnd,
            }
        }),
    )
}

fn act_scroll_target_foreground_guard(
    params: &ActScrollParams,
) -> Result<BackgroundTargetForegroundGuard, ErrorData> {
    let Some(target) = params.target.as_ref() else {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "act_scroll foreground guard requires target.element_id",
        ));
    };
    let hwnd = target
        .element_id
        .parts()
        .map_err(|error| {
            mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "act_scroll element id {} could not be parsed for foreground guard: {error}",
                    target.element_id
                ),
            )
        })?
        .hwnd;
    let root_hwnd = crate::m2::live_target_root(
        hwnd,
        &format!(
            "act_scroll element id {} target window check",
            target.element_id
        ),
    )?;
    Ok(BackgroundTargetForegroundGuard {
        element_hwnd: hwnd,
        root_hwnd,
    })
}

/// What a guarded action's successful reply tells the background foreground guard.
struct BackgroundGuardReply<'a> {
    required_foreground: &'a mut bool,
    source_of_truth: Option<&'a str>,
    backend_tier_used: &'a str,
}

async fn with_background_foreground_guard<T>(
    tool: &'static str,
    session_id: &str,
    guard: Option<BackgroundForegroundSection>,
    action_source_fallback: &'static str,
    run: impl std::future::Future<Output = Result<T, ErrorData>>,
    reply: impl FnOnce(&mut T) -> Option<BackgroundGuardReply<'_>>,
) -> Result<ForegroundGuardedResponse<T>, ErrorData> {
    let Some(guard) = guard else {
        return run.await.map(Into::into);
    };
    let target = guard.target;
    let before = read_background_foreground(tool, "before", action_source_fallback)?;
    let mut result = run.await;
    let (required_foreground, action_source_of_truth, backend_tier_used) = match &mut result {
        Ok(response) => {
            let Some(reply) = reply(response) else {
                return result.map(Into::into);
            };
            (
                Some(reply.required_foreground),
                reply
                    .source_of_truth
                    .unwrap_or(action_source_fallback)
                    .to_owned(),
                Some(reply.backend_tier_used),
            )
        }
        Err(error) => (
            None,
            background_error_source_of_truth(error)
                .unwrap_or_else(|| action_source_fallback.to_owned()),
            None,
        ),
    };
    if !background_guard_applies(required_foreground.as_deref().copied()) {
        return result.map(Into::into);
    }

    // #318c: a posted action reaches the target's message loop after the call
    // returned; wait the bounded settle so a delayed self-activation is seen.
    let configured_settle_ms = background_foreground_settle_config_ms(
        std::env::var(BACKGROUND_FOREGROUND_SETTLE_ENV)
            .ok()
            .as_deref(),
    );
    let settle_ms = background_foreground_settle_ms(backend_tier_used, configured_settle_ms);
    let settle_session_id = session_id.to_owned();
    let settle_source_of_truth = action_source_of_truth.clone();
    // Dropping the caller's JoinHandle detaches this task; keep the section
    // owned by the task until the foreground read and restore have finished.
    let settled = tokio::spawn(async move {
        let _guard = guard;
        if settle_ms > 0 {
            tokio::time::sleep(Duration::from_millis(settle_ms)).await;
        }
        let after = read_background_foreground(tool, "after", &settle_source_of_truth)?;
        let (outcome, foreground) = settle_native_pattern_foreground(
            tool,
            &settle_session_id,
            target.root_hwnd,
            before,
            after,
        );
        Ok::<_, ErrorData>((after, outcome, foreground))
    })
    .await;
    let (after, outcome, foreground) = match settled {
        Ok(Ok(settled)) => settled,
        Ok(Err(error)) => return Err(merge_background_foreground_error(result, error)),
        Err(error) => {
            let error = background_foreground_read_error(
                tool,
                "after",
                &action_source_of_truth,
                &synapse_a11y::A11yError::Internal {
                    detail: error.to_string(),
                },
            );
            return Err(merge_background_foreground_error(result, error));
        }
    };
    if let Some(required) = required_foreground {
        *required = foreground.required_foreground();
    }
    let result = result.map_err(|mut error| {
        let data = error.data.get_or_insert_with(|| json!({}));
        foreground.merge_into(data);
        error
    });
    if let Some(code) = background_click_foreground_error(outcome, foreground.restored()) {
        let error = background_click_foreground_lost_error(
            tool,
            code,
            &action_source_of_truth,
            target,
            before,
            after,
            &foreground,
        );
        return Err(merge_background_foreground_error(result, error));
    }
    if let NativeForegroundOutcome::MovedElsewhere { now } = outcome {
        tracing::warn!(
            code = "BACKGROUND_FOREGROUND_CHANGED_NON_TARGET",
            tool,
            source_of_truth = BACKGROUND_FOREGROUND_SOURCE_OF_TRUTH,
            action_source_of_truth,
            target_element_hwnd = target.element_hwnd,
            target_root_hwnd = target.root_hwnd,
            before_root_hwnd = ?before.map(|identity| identity.root_hwnd),
            after_root_hwnd = ?now,
            "background action completed while foreground changed to a non-target window; not restored"
        );
    }
    result.map(|action| ForegroundGuardedResponse {
        action,
        foreground: Some(foreground),
    })
}

fn background_error_source_of_truth(error: &ErrorData) -> Option<String> {
    let source = error
        .data
        .as_ref()
        .and_then(|data| data.get("source_of_truth"))?;
    if let Some(source) = source.as_str() {
        return Some(source.to_owned());
    }
    if source.is_array() {
        return Some(source.to_string());
    }
    None
}

pub(super) fn merge_background_foreground_error<T>(
    result: Result<T, ErrorData>,
    foreground_error: ErrorData,
) -> ErrorData {
    match result {
        Ok(_) => foreground_error,
        Err(error) => attach_background_foreground_guard_error(error, foreground_error),
    }
}

fn attach_background_foreground_guard_error(
    original: ErrorData,
    foreground_error: ErrorData,
) -> ErrorData {
    let code = original.code;
    let message = original.message.to_string();
    let original_data = original.data;
    let mut data = match original_data {
        Some(Value::Object(map)) => map,
        Some(other) => {
            let mut map = serde_json::Map::new();
            map.insert("original_data".to_owned(), other);
            map
        }
        None => serde_json::Map::new(),
    };
    data.insert(
        "background_foreground_guard".to_owned(),
        json!({
            "code": foreground_error
                .data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(Value::as_str)
                .unwrap_or(error_codes::ACTION_FOREGROUND_CONTEXT_CAPTURE_FAILED),
            "message": foreground_error.message.to_string(),
            "data": foreground_error.data,
        }),
    );
    ErrorData::new(code, message, Some(Value::Object(data)))
}

/// The ACTION_FOREGROUND_LOST error for a background action whose target took the OS
/// foreground from a prior window that could not be put back.
pub(super) fn background_click_foreground_lost_error(
    tool: &'static str,
    code: &'static str,
    action_source_of_truth: &str,
    target: BackgroundTargetForegroundGuard,
    before: Option<synapse_a11y::WindowRootIdentity>,
    after: Option<synapse_a11y::WindowRootIdentity>,
    foreground: &NativeForegroundEvidence,
) -> ErrorData {
    let restore = match foreground.restored() {
        Some(_) => "the restore failed",
        None => "no restore ran",
    };
    let root_hex = |identity: Option<synapse_a11y::WindowRootIdentity>| {
        identity.map_or_else(|| "none".to_owned(), |id| format!("0x{:x}", id.root_hwnd))
    };
    let before_details = window_root_details(before);
    let after_details = window_root_details(after);
    let detail = format!(
        "{tool}: the target took the OS foreground from root hwnd {} ({}) to root hwnd {} ({}) and {restore}",
        root_hex(before),
        before_details["process_name"].as_str().unwrap_or("unknown"),
        root_hex(after),
        after_details["process_name"].as_str().unwrap_or("unknown"),
    );
    tracing::error!(
        code,
        reason = "background_action_changed_foreground",
        tool,
        source_of_truth = BACKGROUND_FOREGROUND_SOURCE_OF_TRUTH,
        action_source_of_truth,
        target_element_hwnd = target.element_hwnd,
        target_root_hwnd = target.root_hwnd,
        %detail,
        "background action target took the foreground and the prior window was not restored"
    );
    let mut data = json!({
        "code": code,
        "reason": "background_action_changed_foreground",
        "tool": tool,
        "source_of_truth": BACKGROUND_FOREGROUND_SOURCE_OF_TRUTH,
        "action_source_of_truth": action_source_of_truth,
        "target_element_hwnd": target.element_hwnd,
        "target_root_hwnd": target.root_hwnd,
        "detail": detail,
        "foreground_before": before_details,
        "foreground_after": after_details,
    });
    foreground.merge_into(&mut data);
    ErrorData::new(ErrorCode(-32099), detail, Some(data))
}

/// Process/window facts for a foreground root, for error evidence. Falls back to the
/// root hwnd and pid when the window can no longer be read.
fn window_root_details(identity: Option<synapse_a11y::WindowRootIdentity>) -> Value {
    let Some(identity) = identity else {
        return json!({ "hwnd": null });
    };
    synapse_a11y::foreground_context(identity.root_hwnd).map_or_else(
        |_| json!({ "hwnd": identity.root_hwnd, "pid": identity.pid }),
        |context| foreground_context_details(&context),
    )
}

fn read_background_foreground(
    tool: &'static str,
    stage: &'static str,
    action_source_of_truth: &str,
) -> Result<Option<synapse_a11y::WindowRootIdentity>, ErrorData> {
    match synapse_a11y::foreground_root_identity() {
        Ok(identity) => Ok(Some(identity)),
        Err(error) if !foreground_read_blocks_background_action(error.code()) => Ok(None),
        Err(error) => Err(background_foreground_read_error(
            tool,
            stage,
            action_source_of_truth,
            &error,
        )),
    }
}

fn background_foreground_read_error(
    tool: &'static str,
    stage: &'static str,
    action_source_of_truth: &str,
    error: &synapse_a11y::A11yError,
) -> ErrorData {
    let detail = format!("{tool} could not read foreground {stage} background dispatch: {error}");
    tracing::error!(
        code = error_codes::ACTION_FOREGROUND_CONTEXT_CAPTURE_FAILED,
        reason = "background_foreground_read_failed",
        tool,
        source_of_truth = BACKGROUND_FOREGROUND_SOURCE_OF_TRUTH,
        action_source_of_truth,
        stage,
        read_error = %error,
        "background foreground guard could not read OS foreground Source of Truth"
    );
    ErrorData::new(
        ErrorCode(-32099),
        detail.clone(),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_CONTEXT_CAPTURE_FAILED,
            "reason": "background_foreground_read_failed",
            "tool": tool,
            "source_of_truth": BACKGROUND_FOREGROUND_SOURCE_OF_TRUTH,
            "action_source_of_truth": action_source_of_truth,
            "stage": stage,
            "detail": detail,
            "read_error": { "code": error.code() },
        })),
    )
}

/// Dynamics the Chromium foreground set_field replacement types with; the same
/// value feeds the #50 foreground budget so projection matches delivery.
const fn set_field_foreground_replace_dynamics() -> KeystrokeDynamics {
    KeystrokeDynamics::Natural {
        params: KeystrokeNaturalParams::FAST,
    }
}

fn act_stroke_foreground_lost_error(
    expected: &ForegroundProof,
    actual: Option<&ForegroundContext>,
    read_error: Option<&ErrorData>,
) -> ErrorData {
    let detail = match actual {
        Some(actual) => format!(
            "act_stroke expected foreground hwnd 0x{:x} ({}) but current foreground is hwnd 0x{:x} ({})",
            expected.hwnd, expected.window_title, actual.hwnd, actual.window_title
        ),
        None => format!(
            "act_stroke could not read current foreground mid-stroke for expected hwnd 0x{:x} ({})",
            expected.hwnd, expected.window_title
        ),
    };
    ErrorData::new(
        ErrorCode(-32099),
        detail.clone(),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_LOST,
            "reason": "act_stroke_foreground_lost_mid_stroke",
            "detail": detail,
            "point_index": Value::Null,
            "queue_rate_state": {
                "kind": "not_rate_or_queue",
            },
            "foreground_expected": expected,
            "foreground_actual": actual.map(foreground_context_details),
            "foreground_read_error": read_error.map(|error| json!({
                "message": error.message.to_string(),
                "data": error.data.clone(),
            })),
        })),
    )
}

fn foreground_context_details(foreground: &ForegroundContext) -> Value {
    json!({
        "hwnd": foreground.hwnd,
        "pid": foreground.pid,
        "process_name": &foreground.process_name,
        "process_path": &foreground.process_path,
        "window_title": &foreground.window_title,
        "monitor_index": foreground.monitor_index,
        "dpi_scale": foreground.dpi_scale,
        "profile_id": &foreground.profile_id,
        "steam_appid": foreground.steam_appid,
        "is_fullscreen": foreground.is_fullscreen,
        "is_dwm_composed": foreground.is_dwm_composed,
        "window_bounds": &foreground.window_bounds,
    })
}

fn clipboard_response_audit_details(response: &ActClipboardResponse) -> Value {
    json!({
        "response": {
            "ok": response.ok,
            "verb": response.verb,
            "format": response.format,
            "written": response.written,
            "cleared": response.cleared,
            "text_len": response.text_len,
            "text_present": response.text.is_some(),
            "backing": response.backing,
            "backend_tier_used": response.backend_tier_used,
            "source_of_truth": response.source_of_truth,
            "os_clipboard_touched": response.os_clipboard_touched,
            "required_foreground": response.required_foreground,
            "lease_required": response.lease_required,
            "elapsed_ms": response.elapsed_ms,
        },
    })
}

fn clipboard_request_audit_details(params: &ActClipboardParams) -> Value {
    json!({
        "verb": params.verb,
        "format": params.format,
        "text_len": params.text.as_ref().map(|text| text.chars().count()),
        "session_scoped": true,
        "source_of_truth": "session_clipboard_buffer",
        "os_clipboard_touched": false,
        "backend_tier_used": "session_buffer",
        "required_foreground": false,
        "lease_required": false,
    })
}

fn ensure_everquest_click_backend(
    params: &ActClickParams,
    preflight: &ActionPreflightReadback,
) -> Result<(), ErrorData> {
    if preflight.target_profile_id.as_deref() == Some("everquest.live")
        && params.backend == Backend::Software
    {
        return Err(mcp_error(
            error_codes::ACTION_BACKEND_UNAVAILABLE,
            "everquest.live software mouse clicks are not FSV-accepted; use backend=hardware through the configured HID path or a keyboard keymap equivalent",
        ));
    }
    Ok(())
}

#[path = "act_type_date_parent.rs"]
mod act_type_date_parent;

#[path = "m2_pixel_signature.rs"]
mod pixel_signature;

#[path = "m2_click_effect.rs"]
mod click_effect;
use click_effect::click_effect_changed;

#[cfg(test)]
#[path = "tests/segmented_date_type.rs"]
mod segmented_date_type_tests;

#[cfg(test)]
#[path = "tests/segmented_date_type_80b.rs"]
mod segmented_date_type_80b_tests;

#[cfg(test)]
#[path = "tests/date_parent_live_lookup_80.rs"]
mod date_parent_live_lookup_80_tests;

#[cfg(test)]
#[path = "tests/coordinate_click_capture_81.rs"]
mod coordinate_click_capture_81_tests;

#[cfg(test)]
#[path = "tests/related_window_live_evidence_92.rs"]
mod related_window_live_evidence_92_tests;

#[cfg(test)]
#[path = "tests/type_fallback_click_gate_194.rs"]
mod type_fallback_click_gate_194_tests;

#[cfg(all(test, windows))]
#[path = "tests/webview_click_preflight_probes_74.rs"]
mod webview_click_preflight_probes_74_tests;

#[cfg(all(test, windows))]
#[path = "tests/stroke_claim_dispatch_202.rs"]
mod stroke_claim_dispatch_202_tests;

#[cfg(test)]
#[path = "tests/input_p3.rs"]
mod input_p3_tests;

#[cfg(test)]
#[path = "tests/related_window.rs"]
mod related_window_tests;

#[cfg(test)]
#[path = "tests/related_window_child_process_92.rs"]
mod related_window_child_process_92;

#[cfg(test)]
#[path = "tests/webview2_point_click_preflight_79.rs"]
mod webview2_point_click_preflight_79_tests;

#[cfg(test)]
#[path = "tests/queue_full_lease_195.rs"]
mod queue_full_lease_195_tests;

#[path = "foreground_click_actionability.rs"]
pub(crate) mod foreground_click_actionability;

#[cfg(test)]
#[path = "tests/click_actionability_194.rs"]
mod click_actionability_194_tests;

#[cfg(test)]
#[path = "tests/window_pixel_signature_191.rs"]
mod window_pixel_signature_191_tests;

#[cfg(test)]
#[path = "tests/click_router_no_redelivery_189.rs"]
mod click_router_no_redelivery_189_tests;

#[cfg(test)]
#[path = "tests/click_delta_readback_218.rs"]
mod click_delta_readback_218_tests;

#[cfg(test)]
#[path = "tests/click_focus_only_107.rs"]
mod click_focus_only_107_tests;

#[cfg(test)]
#[path = "tests/click_pixel_evidence_191_218.rs"]
mod click_pixel_evidence_191_218_tests;

#[cfg(test)]
#[path = "tests/web_element_claim_target_202.rs"]
mod web_element_claim_target_202_tests;

#[cfg(test)]
#[path = "tests/stroke_claim_targets_202.rs"]
mod stroke_claim_targets_202_tests;

#[cfg(test)]
#[path = "tests/scroll_point_capture_191.rs"]
mod scroll_point_capture_191_tests;

#[cfg(test)]
#[path = "tests/m2_tools_source_scan.rs"]
mod m2_tools_source_scan;

#[cfg(test)]
#[path = "tests/unproven_delivery_no_delta_218.rs"]
mod unproven_delivery_no_delta_218_tests;

#[cfg(test)]
#[path = "tests/reconciled_click_attempt_190.rs"]
mod reconciled_click_attempt_190_tests;

async fn queue_full_setup_with_lease(
    lease: &mut crate::m2::ForegroundInputLeaseGuard,
    handle: &synapse_action::ActionHandle,
    from: Point,
    blocker_duration_ms: u32,
) -> Result<ActionDiagnosticQueueFullSetupResponse, ErrorData> {
    validate_queue_blocker_duration_ms(blocker_duration_ms)?;
    let to = diagnostic_adjacent_point(from);
    handle
        .try_execute(diagnostic_queue_blocker_action(
            from,
            to,
            blocker_duration_ms,
        ))
        .map_err(diagnostic_action_error_to_mcp)?;
    // Keep the lease only after the blocker was accepted (#195).
    lease.hold_until_ttl_expiry("queue_full_blocker_still_running");
    tokio::time::sleep(Duration::from_millis(ACTION_DIAGNOSTIC_QUEUE_SETTLE_MS)).await;
    let (filler_attempts, queued_fillers, queue_full_observed) = saturate_action_queue(handle)?;
    if !queue_full_observed {
        return Err(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!(
                "action_diagnostic_queue_full_setup failed to observe ACTION_QUEUE_FULL after {filler_attempts} attempts and {queued_fillers} queued fillers"
            ),
        ));
    }
    Ok(ActionDiagnosticQueueFullSetupResponse {
        backend: ResolvedBackend::Software.as_str().to_owned(),
        expected_queue_capacity: u32::try_from(ACTION_QUEUE_CAPACITY).unwrap_or(u32::MAX),
        blocker_duration_ms,
        blocker_from: from,
        blocker_to: to,
        blocker_queued: true,
        filler_attempts,
        queued_fillers,
        queue_full_observed,
        next_act_stroke_expected_error: error_codes::ACTION_QUEUE_FULL.to_owned(),
    })
}

#[cfg(test)]
#[path = "tests/click_signature_support.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/click_router_deliveries_189_190.rs"]
mod click_router_deliveries_189_190;

#[cfg(test)]
#[path = "tests/click_router_refusal_189.rs"]
mod click_router_refusal_189;

#[cfg(all(test, windows))]
#[path = "tests/occluded_pixel_evidence_191.rs"]
mod occluded_pixel_evidence_191;

#[cfg(test)]
#[path = "tests/queue_full_lease_hold_195.rs"]
mod queue_full_lease_hold_195;

#[cfg(all(test, windows))]
#[path = "tests/own_window_no_delta_218.rs"]
mod own_window_no_delta_218;

#[cfg(all(test, windows))]
#[path = "tests/webview2_preflight_injected_79.rs"]
mod webview2_preflight_injected_79_tests;

#[cfg(test)]
#[path = "tests/focus_recovery_gate_6.rs"]
mod focus_recovery_gate_6_tests;

#[cfg(test)]
#[path = "tests/claimed_type_target_108.rs"]
mod claimed_type_target_108_tests;

#[cfg(test)]
#[path = "tests/inner_gate_routes_258.rs"]
mod inner_gate_routes_258;

#[cfg(all(test, windows))]
#[path = "tests/explicit_press_claim_root_62.rs"]
mod explicit_press_claim_root_62;
