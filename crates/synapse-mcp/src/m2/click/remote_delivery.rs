//! Preserve remote click uncertainty across transport and later readback failures.
use super::element::{click_attempts, mark_click_not_dispatched, stop_click_router};
use super::*;
use crate::m2::postcondition::{mark_input_delivered, mark_input_possibly_delivered};

/// The remote call may send input before its reply fails. Only explicit
/// pre-dispatch evidence permits treating its failure as undelivered.
pub(crate) async fn remote_click_with<T>(
    dispatch: impl std::future::Future<Output = Result<T, ErrorData>>,
) -> Result<T, ErrorData> {
    dispatch
        .await
        .map_err(|error| remote_click_error(error, false))
}

/// Classify the remote call's own failure. A proven pre-dispatch failure stays
/// a refusal; any other failure may have reached the page.
pub(crate) fn remote_click_error(error: ErrorData, delivered: bool) -> ErrorData {
    let delivered = delivered
        || error
            .data
            .as_ref()
            .is_some_and(|data| data["delivered"] == true);
    if !delivered && remote_presend_proof(&error) {
        return mark_click_not_dispatched(error);
    }
    remote_delivery_evidence(error, delivered)
}

/// Raw-CDP producers type `mousePressed`/`mouseReleased` failures as
/// `ClickDispatch` (released-after-pressed is delivered). Connect, target/node
/// resolution, box model, point validation and `mouseMoved` fail before any
/// button event is sent, so they are proven undelivered.
pub(crate) fn remote_cdp_click_failure(
    source: &synapse_a11y::A11yError,
    error: ErrorData,
) -> ErrorData {
    match source {
        synapse_a11y::A11yError::ClickDispatch {
            delivered: true, ..
        } => mark_input_delivered(error),
        synapse_a11y::A11yError::ClickDispatch { .. } => error,
        _ => mark_click_not_dispatched(error),
    }
}

/// #189 R5: a bridge click failure carries the dispatch stage its producer
/// (daemon enqueue or extension) proved; without one, delivery is unknown.
pub(crate) fn remote_bridge_click_error(
    error: &crate::chrome_debugger_bridge::ChromeDebuggerBridgeError,
) -> ErrorData {
    remote_click_error(
        stamp_bridge_dispatch_stage(
            crate::m1::mcp_error(error.code(), error.detail().to_owned()),
            error.dispatch_stage(),
        ),
        false,
    )
}

pub(crate) fn stamp_bridge_dispatch_stage(
    error: ErrorData,
    stage: crate::chrome_debugger_bridge::BridgeDispatchStage,
) -> ErrorData {
    use crate::chrome_debugger_bridge::BridgeDispatchStage;
    match stage {
        BridgeDispatchStage::NotStarted => mark_click_not_dispatched(error),
        BridgeDispatchStage::AfterDispatch => mark_input_delivered(error),
        BridgeDispatchStage::Unreported => error,
    }
}

fn remote_delivery_evidence(mut error: ErrorData, delivered: bool) -> ErrorData {
    let mut attempts = click_attempts(&error);
    if attempts.is_empty() {
        attempts.push(click_tier_failed(
            CLICK_TIER_CDP,
            CLICK_REASON_DELIVERY_UNKNOWN,
            click_error_code(&error),
            false,
            error.message.to_string(),
        ));
    }
    error = stop_click_router(error, "remote_click_delivery_unverified");
    error = if delivered {
        mark_input_delivered(error)
    } else {
        mark_input_possibly_delivered(error)
    };
    attach_click_tier_attempts(error, attempts)
}

fn remote_presend_proof(error: &ErrorData) -> bool {
    if error
        .data
        .as_ref()
        .is_some_and(|data| data["dispatch_not_started"] == true)
    {
        return true;
    }
    let code = click_error_code(error);
    remote_predispatch_code(&code)
        // send_attach_command always refuses this unsupported bridge lane before enqueue.
        || (code == error_codes::A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED
            && error.message.contains("refused unsupported attach-capable command")
            && error.message.contains("before queueing any Chrome command"))
}

/// Codes whose every producer runs before the first input event:
/// - DOM locator/actionability codes: the extension resolves and preflights the
///   target (domAction, cdpInput, coordinateClick) before any pointer event.
///   `CHROME_DOM_ACTION_UNSUPPORTED` is absent: it is also the extension's
///   catch-all after events were dispatched, so only an explicit
///   `dispatch_stage=not_started` proves it pre-dispatch (#189 B8);
/// - `BROWSER_WAIT_TIMEOUT`: auto-wait preflight only (post-click page waits
///   report `A11Y_CDP_EXTENSION_TIMEOUT`);
/// - bridge unavailable / stale / capability: refused before the command is
///   queued, or by the extension's command switch before any handler runs;
/// - `TOOL_PARAMS_INVALID`: daemon-side validation before enqueue.
///
/// Timeout, detach, attach/protocol, scripting and AX-tree codes are absent on
/// purpose: their producers also run after input was dispatched.
fn remote_predispatch_code(code: &str) -> bool {
    matches!(
        code,
        error_codes::CHROME_DOM_SELECTOR_INVALID
            | error_codes::CHROME_DOM_ELEMENT_NOT_FOUND
            | error_codes::CHROME_DOM_ELEMENT_AMBIGUOUS
            | error_codes::CHROME_DOM_ELEMENT_NOT_ACTIONABLE
            | error_codes::BROWSER_WAIT_TIMEOUT
            | error_codes::A11Y_CDP_EXTENSION_UNAVAILABLE
            | error_codes::CHROME_BRIDGE_EXTENSION_STALE
            | error_codes::BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED
            | error_codes::TOOL_PARAMS_INVALID
    )
}

/// An audit/redaction failure must retain the physical click's delivery state.
/// The follow-up error never proves anything about dispatch, so no pre-send
/// classification is applied to it.
pub(crate) fn carry_remote_click_delivery<T>(
    error: ErrorData,
    result: &Result<T, ErrorData>,
) -> ErrorData {
    match result {
        Ok(_) => remote_delivery_evidence(error, true),
        Err(original) => {
            let Some(data) = original.data.as_ref() else {
                return error;
            };
            let delivered = data["delivered"] == true;
            if !delivered && data["possibly_delivered"] != true {
                return error;
            }
            let error = attach_click_tier_attempts(error, click_attempts(original));
            remote_delivery_evidence(error, delivered)
        }
    }
}

#[cfg(test)]
#[path = "tests/remote_delivery_189.rs"]
mod remote_delivery_189;

#[cfg(test)]
#[path = "tests/delivered_attempt_wire_271.rs"]
mod delivered_attempt_wire_271;
