use super::*;

/// Terminal errors retain the original code and evidence but never authorize replay.
pub(crate) fn stop_click_router(mut error: ErrorData, reason: &'static str) -> ErrorData {
    if let Value::Object(data) = error.data.get_or_insert_with(|| json!({})) {
        data.insert("router_escalation_allowed".to_owned(), json!(false));
        data.insert("router_escalation_required".to_owned(), json!(false));
        data.insert("router_next_tier".to_owned(), Value::Null);
        data.insert("router_stop_reason".to_owned(), json!(reason));
    }
    error
}

pub(crate) fn click_router_stopped(error: &ErrorData) -> bool {
    error.data.as_ref().is_some_and(|data| {
        data["router_escalation_allowed"] == false
            || data["delivered"] == true
            || data["possibly_delivered"] == true
            || data["tier_attempts"].as_array().is_some_and(|attempts| {
                attempts
                    .iter()
                    .any(|attempt| attempt["status"] == "delivered")
            })
    })
}

pub(crate) fn mark_click_not_dispatched(mut error: ErrorData) -> ErrorData {
    if !click_router_stopped(&error)
        && let Value::Object(data) = error.data.get_or_insert_with(|| json!({}))
    {
        data.insert("dispatch_not_started".to_owned(), json!(true));
    }
    error
}

/// #189 R4: a failed gate read (metadata, caret route) is a terminal refusal:
/// proven undelivered, and never a reason to try another route.
pub(crate) fn gate_read_failed(error: ErrorData) -> ErrorData {
    stop_click_router(mark_click_not_dispatched(error), "gate_read_failed")
}

/// #189 R5: delivery evidence is the producer's typed wrapper, never an error
/// code or text. `None` is proven not dispatched; an error from the
/// dispatching call without a wrapper is unknown delivery.
fn dispatch_evidence(error: &ActionError) -> Option<(bool, &'static str)> {
    match error {
        ActionError::ClickNotDispatched { .. } => None,
        ActionError::ClickDispatch {
            delivered: true, ..
        } => Some((true, "uia_readback_after_dispatch")),
        ActionError::ClickDispatch { .. } => Some((false, "uia_provider_delivery_unknown")),
        _ => Some((false, "uia_untyped_delivery_unknown")),
    }
}

/// #189 U9: the UIA worker refused admission, so the UIA tier never ran.
pub(crate) const fn uia_tier_unavailable(error: &ActionError) -> bool {
    matches!(
        error,
        ActionError::ClickNotDispatched {
            tier_unavailable: true,
            ..
        }
    )
}

pub(crate) fn uia_click_error_after_dispatch(
    error: &ActionError,
    index: u8,
    clicks: u8,
    outcomes: &[String],
) -> Option<ErrorData> {
    let evidence = dispatch_evidence(error);
    if index == 0 && evidence.is_none() {
        return None;
    }
    let delivered = index > 0 || evidence.is_some_and(|(delivered, _)| delivered);
    let reason = if index > 0 {
        "uia_partial_multi_click_delivered"
    } else {
        evidence.map_or("uia_provider_delivery_unknown", |(_, reason)| reason)
    };
    let attempts = uia_failure_attempts(error, index, clicks, outcomes, delivered);
    // Stale-element pre-dispatch guidance would be false after a dispatched Invoke.
    let converted = crate::m2::action_error_to_mcp(error);
    let mut converted = attach_click_tier_attempts(converted, attempts);
    if evidence.is_some_and(|(delivered, _)| delivered)
        && let Some(data) = converted.data.as_mut().and_then(Value::as_object_mut)
    {
        data.insert("click_dispatch_stage".to_owned(), json!("readback"));
    }
    let marked = if delivered {
        mark_input_delivered(converted)
    } else {
        mark_input_possibly_delivered(converted)
    };
    tracing::warn!(
        code = "M2_ACT_CLICK_UIA_ERROR_AFTER_DISPATCH",
        kind = "act_click",
        click_index = index,
        clicks,
        stop_reason = reason,
        delivered,
        detail = error.detail(),
        "act_click UIA click failed after input may have been delivered; no fallback or router escalation"
    );
    Some(stop_click_router(marked, reason))
}

fn uia_failure_attempts(
    error: &ActionError,
    index: u8,
    clicks: u8,
    outcomes: &[String],
    delivered: bool,
) -> Vec<ActClickTierAttempt> {
    let mut attempts = Vec::new();
    if delivered {
        attempts.push(click_tier_delivered(
            CLICK_TIER_UIA,
            false,
            format!(
                "UIA dispatched before failure; completed={index}/{clicks}; outcomes={}",
                outcomes.join(",")
            ),
        ));
    }
    attempts.push(click_tier_failed(
        CLICK_TIER_UIA,
        CLICK_REASON_DELIVERY_UNKNOWN,
        error.code(),
        false,
        error.detail(),
    ));
    attempts
}

pub(crate) fn click_metadata_refusal_error(error: &ActionError) -> ErrorData {
    let error_code = error.code();
    let error_data = stop_click_router(
        action_error_to_mcp(error),
        "coordinate_fallback_metadata_denied",
    );
    attach_click_tier_attempts(
        error_data,
        vec![click_tier_failed(
            CLICK_TIER_UIA,
            click_reason_for_error_code(error_code),
            error_code,
            false,
            error.detail(),
        )],
    )
}

/// Both PostMessage wrappers use this attachment, including partial delivery.
pub(crate) fn postmessage_failure(
    mut error: ErrorData,
    mut prior: Vec<ActClickTierAttempt>,
) -> ErrorData {
    if !click_router_stopped(&error)
        && !error
            .data
            .as_ref()
            .is_some_and(|data| data["dispatch_not_started"] == true)
    {
        error = mark_input_possibly_delivered(error);
    }

    if error
        .data
        .as_ref()
        .is_some_and(|data| data["delivered"] == true)
    {
        prior.push(click_tier_delivered(
            CLICK_TIER_POSTMESSAGE,
            false,
            "button message queued before failure",
        ));
    }
    prior.push(click_tier_failed(
        CLICK_TIER_POSTMESSAGE,
        click_reason_for_error_code(&click_error_code(&error)),
        click_error_code(&error),
        false,
        error.message.to_string(),
    ));
    attach_click_tier_attempts(error, prior)
}

pub(crate) fn click_attempts(error: &ErrorData) -> Vec<ActClickTierAttempt> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("tier_attempts"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}
