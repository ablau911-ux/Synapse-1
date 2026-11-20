use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData, model::ErrorCode};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use synapse_core::error_codes;

pub const DEFAULT_VERIFY_TIMEOUT_MS: u32 = 250;

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActPostcondition {
    pub status: String,
    pub observed_delta: Option<bool>,
    pub source_of_truth: Option<String>,
    pub before_signature: Option<String>,
    pub after_signature: Option<String>,
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_window_observed: Option<RelatedWindowObservation>,
}

/// A newly inventoried foreground window with verified ownership, not causal attribution.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelatedWindowObservation {
    pub hwnd: i64,
    pub pid: u32,
    pub title: String,
    pub target_hwnd: i64,
    pub relationship: String,
}

pub const fn default_verify_timeout_ms() -> u32 {
    DEFAULT_VERIFY_TIMEOUT_MS
}

pub fn postcondition_not_requested(tool: &str, source_of_truth: &str) -> ActPostcondition {
    ActPostcondition {
        related_window_observed: None,
        status: "not_requested".to_owned(),
        observed_delta: None,
        source_of_truth: Some(source_of_truth.to_owned()),
        before_signature: None,
        after_signature: None,
        detail: Some(format!(
            "{tool} only verified low-level delivery; set verify_delta=true to require an observed Source-of-Truth delta"
        )),
    }
}

pub fn postcondition_observed_delta(
    tool: &str,
    source_of_truth: &str,
    before_signature: String,
    after_signature: String,
    detail: impl Into<String>,
) -> ActPostcondition {
    ActPostcondition {
        related_window_observed: None,
        status: "observed_delta".to_owned(),
        observed_delta: Some(true),
        source_of_truth: Some(source_of_truth.to_owned()),
        before_signature: Some(before_signature),
        after_signature: Some(after_signature),
        detail: Some(format!("{tool} verify_delta {}", detail.into())),
    }
}

/// #1360: a DELIVERED act_click that closed its own target window (a dialog
/// Open/OK/Cancel button) makes the post-delivery Source-of-Truth readback fail
/// because the window is gone — but the click succeeded: the window
/// disappearing IS the observed delta and the click's intended effect. Verified
/// via target-window liveness, not reported as a false-negative refusal.
pub fn postcondition_target_window_closed(
    tool: &str,
    detail: impl Into<String>,
) -> ActPostcondition {
    ActPostcondition {
        related_window_observed: None,
        status: "observed_delta".to_owned(),
        observed_delta: Some(true),
        source_of_truth: Some("target_window_liveness".to_owned()),
        before_signature: Some("target_window_live".to_owned()),
        after_signature: Some("target_window_closed".to_owned()),
        detail: Some(format!(
            "{tool} verify_delta: target window closed after a delivered click (the click dismissed it); {}",
            detail.into()
        )),
    }
}

pub fn no_observed_delta_error(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before_signature: String,
    after_signature: String,
    readback: Value,
) -> ErrorData {
    tracing::warn!(
        code = error_codes::ACTION_NO_OBSERVED_DELTA,
        tool,
        source_of_truth,
        timeout_ms,
        before_signature,
        after_signature,
        readback = %readback,
        "verify_delta observed no Source-of-Truth state change"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} verify_delta observed no Source-of-Truth state change within {timeout_ms} ms"
        ),
        Some(json!({
            "code": error_codes::ACTION_NO_OBSERVED_DELTA,
            "tool": tool,
            "source_of_truth": source_of_truth,
            "verify_delta": {
                "timeout_ms": timeout_ms,
                "before_signature": before_signature,
                "after_signature": after_signature,
                "readback": readback,
            }
        })),
    )
}

/// Hint attached to a no-delta error whose input was already delivered (#218).
pub const DELIVERED_NO_DELTA_RETRY_HINT: &str = "input was delivered but its effect was not observed; re-observe the target before acting again instead of retrying blindly";

/// `no_observed_delta_error` for input that verifiably reached the target
/// (#218): the action is delivered-but-unverified, not undelivered, so the
/// error data says `delivered: true` and warns against a blind retry that could
/// apply the action twice.
pub fn delivered_no_observed_delta_error(
    tool: &str,
    source_of_truth: &str,
    timeout_ms: u32,
    before_signature: String,
    after_signature: String,
    readback: Value,
) -> ErrorData {
    mark_input_delivered(no_observed_delta_error(
        tool,
        source_of_truth,
        timeout_ms,
        before_signature,
        after_signature,
        readback,
    ))
}

/// Marks a post-delivery verification failure (#189/#218): `delivered: true`
/// plus the no-blind-retry hint, so a caller never repeats an action that
/// already happened just because its effect was not seen.
pub fn mark_input_delivered(mut error: ErrorData) -> ErrorData {
    // #189: a marker is evidence; never drop it because the error had no data.
    if let Value::Object(data) = error.data.get_or_insert_with(|| json!({})) {
        data.insert("delivered".to_owned(), Value::Bool(true));
        data.insert(
            "retry_hint".to_owned(),
            Value::String(DELIVERED_NO_DELTA_RETRY_HINT.to_owned()),
        );
    }
    error
}

/// Hint attached to an error whose input may already have been delivered (#189).
pub const POSSIBLY_DELIVERED_RETRY_HINT: &str = "the input may already have been delivered (the delivery call did not report back); re-observe the target before acting again instead of retrying blindly";

/// Marks a failure after which delivery is unknown (#189), e.g. a UIA Invoke
/// sent to the provider whose reply timed out (blocked on a modal dialog):
/// `possibly_delivered: true` plus the no-blind-retry hint. It never sets
/// `delivered: true`, which would claim a delivery nobody observed.
pub fn mark_input_possibly_delivered(mut error: ErrorData) -> ErrorData {
    if let Value::Object(data) = error.data.get_or_insert_with(|| json!({})) {
        data.insert("possibly_delivered".to_owned(), Value::Bool(true));
        data.insert(
            "retry_hint".to_owned(),
            Value::String(POSSIBLY_DELIVERED_RETRY_HINT.to_owned()),
        );
    }
    error
}

pub fn postcondition_failed_error(
    tool: &str,
    source_of_truth: &str,
    detail: impl Into<String>,
    before_signature: String,
    after_signature: String,
    readback: Value,
) -> ErrorData {
    let detail = detail.into();
    tracing::error!(
        code = error_codes::ACTION_POSTCONDITION_FAILED,
        tool,
        source_of_truth,
        before_signature,
        after_signature,
        detail = %detail,
        readback = %readback,
        "verify_delta Source-of-Truth postcondition failed"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!("{tool} verify_delta Source-of-Truth postcondition failed: {detail}"),
        Some(json!({
            "code": error_codes::ACTION_POSTCONDITION_FAILED,
            "tool": tool,
            "source_of_truth": source_of_truth,
            "detail": detail,
            "verify_delta": {
                "before_signature": before_signature,
                "after_signature": after_signature,
                "readback": readback,
            }
        })),
    )
}

pub fn hash_json<T: Serialize>(value: &T) -> Result<String, ErrorData> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        ErrorData::new(
            ErrorCode(-32099),
            format!("failed to encode verify_delta signature: {error}"),
            Some(json!({
                "code": error_codes::TOOL_INTERNAL_ERROR,
                "detail": error.to_string(),
            })),
        )
    })?;
    Ok(hex_encode(&Sha256::digest(bytes)))
}

/// Keyed (HMAC-SHA256, per-install key) digest of user content; see
/// `crate::content_digest`. Not recomputable by clients.
pub fn text_signature(value: &str) -> String {
    crate::content_digest::keyed_text_digest_hex(value)
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
#[path = "tests/delivered_no_delta_218.rs"]
mod delivered_no_delta_218_tests;

#[cfg(test)]
#[path = "tests/marker_without_data_271.rs"]
mod marker_without_data_271_tests;
