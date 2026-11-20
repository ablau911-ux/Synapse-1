//! Typed, retryable foreground deferral (#9).
//!
//! A real foreground activation can be refused for transient reasons: the
//! workstation is locked (`LockApp.exe` owns the input desktop) or Windows'
//! foreground-lock rules reject `SetForegroundWindow`. Nothing was delivered and
//! the target and claim are untouched, so the caller must be able to retry the
//! same call later instead of treating the QA run as failed or blocked.
//!
//! The classification travels in the `FOREGROUND_ACTIVATION_REFUSED` error data
//! (`retryable=true`); `target_act` then reports `status=deferred`,
//! `delivery_state=deferred_without_delivery` plus the retry hint and the
//! preserved continuation. Policy refusals of the same error code carry no
//! `retryable` flag and keep their `refused` classification.

use rmcp::ErrorData;
use serde_json::{Value, json};
use synapse_core::error_codes;

/// Suggested wait before the caller repeats a deferred foreground call.
pub(crate) const FOREGROUND_RETRY_AFTER_MS: u32 = 1000;

/// Adds the retryable-deferral fields to a `FOREGROUND_ACTIVATION_REFUSED`
/// error `data` object. `lock_screen` is the live lock-state classification.
pub(crate) fn mark_deferred(data: &mut Value, lock_screen: bool, target_hwnd: Option<i64>) {
    let Some(object) = data.as_object_mut() else {
        return;
    };
    object.insert("retryable".to_owned(), Value::Bool(true));
    object.insert(
        "retry_after_ms".to_owned(),
        json!(FOREGROUND_RETRY_AFTER_MS),
    );
    object.insert("lock_screen".to_owned(), Value::Bool(lock_screen));
    object.insert(
        "deferred_reason".to_owned(),
        json!(if lock_screen {
            "workstation_locked"
        } else {
            "foreground_activation_refused"
        }),
    );
    if let Some(hwnd) = target_hwnd {
        object.insert("target_hwnd".to_owned(), json!(hwnd));
    }
}

fn deferral_data(error: &ErrorData) -> Option<&Value> {
    let data = error.data.as_ref()?;
    let refused = data.get("code").and_then(Value::as_str)
        == Some(error_codes::FOREGROUND_ACTIVATION_REFUSED);
    let retryable = data.get("retryable").and_then(Value::as_bool) == Some(true);
    (refused && retryable).then_some(data)
}

/// True when `error` is a retryable foreground refusal (see module docs).
pub(crate) fn is_deferred(error: &ErrorData) -> bool {
    deferral_data(error).is_some()
}

/// Top-level `target_act` result fields for a deferred error: the retry hint and
/// the continuation that names the preserved target. `None` for any other error.
pub(crate) fn deferred_result_fields(error: &ErrorData) -> Option<Value> {
    let data = deferral_data(error)?;
    let retry_after_ms = data
        .get("retry_after_ms")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| u64::from(FOREGROUND_RETRY_AFTER_MS));
    let lock_screen = data.get("lock_screen").and_then(Value::as_bool) == Some(true);
    Some(json!({
        "retryable": true,
        "retry_after_ms": retry_after_ms,
        "continuation": {
            "preserved": true,
            "target_hwnd": data.get("target_hwnd").cloned().unwrap_or(Value::Null),
            "lock_screen": lock_screen,
            "reason": data.get("deferred_reason").cloned().unwrap_or(Value::Null),
            "guidance": "Nothing was delivered. The session target and claim are unchanged; repeat the same call after retry_after_ms. Target-scoped background verbs need no foreground and can proceed meanwhile.",
        },
    }))
}
