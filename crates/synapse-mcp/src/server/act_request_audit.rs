//! Sanitized request identity for act_* `started` audit rows (#51).
//!
//! Every foreground/background input verb records *what* it was asked to do
//! in CF_ACTION_LOG, so an auditor can reconstruct a leased action from the
//! log alone. Geometry and recognized named keys are recorded verbatim; typed
//! text is recorded only as its length and sha256, and printable or
//! unrecognized keystrokes outside a shortcut chord are redacted outright, so
//! the log can never reassemble typed secrets. Every
//! variable-length field is bounded so a rejected request cannot grow the log.

use serde_json::{Value, json};

use super::{ActClickParams, ActPadParams, ActPressParams, ActScrollParams, ActTypeParams};
use crate::m2::ActClickTarget;
use crate::m2::postcondition::text_signature;

const MAX_AUDIT_KEYS: usize = 32;
const MAX_AUDIT_LIST: usize = 16;
const MAX_AUDIT_LABEL_CHARS: usize = 64;
const MAX_AUDIT_ID_CHARS: usize = 256;
const MAX_AUDIT_DEBUG_CHARS: usize = 512;
const REDACTED_PRINTABLE_KEY: &str = "<redacted:printable>";
const REDACTED_UNRECOGNIZED_KEY: &str = "<redacted:unrecognized>";

/// Builds the `request` object stored in an act_* started audit row.
pub(crate) struct ActRequestAudit;

impl ActRequestAudit {
    pub(crate) fn click(params: &ActClickParams) -> Value {
        let target = match &params.target {
            ActClickTarget::Element(element) => {
                json!({ "kind": "element", "element_id": bounded_id(&element.element_id) })
            }
            ActClickTarget::Point(point) => json!({ "kind": "point", "x": point.x, "y": point.y }),
        };
        json!({
            "target": target,
            "button": debug_label(&params.button),
            "clicks": params.clicks,
            "modifiers": params
                .modifiers
                .iter()
                .take(MAX_AUDIT_LIST)
                .map(debug_label)
                .collect::<Vec<_>>(),
            "velocity_profile": debug_label(&params.velocity_profile),
            "duration_ms": params.duration_ms,
            "hold_ms": params.hold_ms,
            "backend": debug_label(&params.backend),
            "use_invoke_pattern": params.use_invoke_pattern,
            "verify_target_window_hwnd": params.verify_target_window_hwnd,
        })
    }

    pub(crate) fn press(params: &ActPressParams) -> Value {
        json!({
            "keys": Self::sanitized_keys(&params.keys),
            "key_count": params.keys.len(),
            "hold_ms": params.hold_ms,
            "backend": debug_label(&params.backend),
            "window_hwnd": params.window_hwnd,
            "cdp_target_id": params
                .cdp_target_id
                .as_deref()
                .map(|id| bounded(id, MAX_AUDIT_ID_CHARS)),
            "auto_wait_element_id": params.auto_wait_element_id.as_ref().map(bounded_id),
        })
    }

    pub(crate) fn type_text(params: &ActTypeParams) -> Value {
        json!({
            "requested_len": params.text.chars().count(),
            "requested_sha256": text_signature(&params.text),
            "into_element": params.into_element.as_ref().map(bounded_id),
            "press_enter_after": params.press_enter_after,
            "backend": debug_label(&params.backend),
            "verify_target_window_hwnd": params.verify_target_window_hwnd,
        })
    }

    pub(crate) fn scroll(params: &ActScrollParams) -> Value {
        json!({
            "dx": params.dx,
            "dy": params.dy,
            "at": params.at.as_ref().map(|point| json!({ "x": point.x, "y": point.y })),
            "element_id": params.target.as_ref().map(|target| bounded_id(&target.element_id)),
            "smooth": params.smooth,
        })
    }

    pub(crate) fn pad(params: &ActPadParams) -> Value {
        json!({
            "pad_id": debug_label(&params.pad_id),
            "controller": debug_label(&params.controller),
            "report": bounded(&debug_label(&params.report), MAX_AUDIT_DEBUG_CHARS),
            "backend": debug_label(&params.backend),
            "hold_ms": params.hold_ms,
        })
    }

    /// Started-row details: the sanitized request next to the preflight readback.
    pub(crate) fn started_details(request: Value, preflight: &impl serde::Serialize) -> Value {
        json!({ "request": request, "preflight": preflight })
    }

    /// Recognized named keys are recorded in canonical form. A text-producing
    /// key (a printable character or `space`) is shown only in a valid
    /// shortcut chord: every key recognized and distinct, exactly one
    /// text-producing key, and a Ctrl, Alt or Windows modifier (but not
    /// Ctrl+Alt, which is AltGr and types text). In any other request,
    /// including one the press parser will reject, text-producing keys are
    /// redacted. Unrecognized names are always redacted, so a mistaken secret
    /// passed as a key name never reaches the log. No hash of the keys is
    /// kept: a single keystroke has too little entropy for a hash to hide it.
    fn sanitized_keys(keys: &[String]) -> Vec<String> {
        // Only the bounded prefix is inspected; an over-long list is never a
        // valid chord, so its text-producing keys stay redacted.
        let names: Vec<Option<String>> = keys
            .iter()
            .take(MAX_AUDIT_KEYS)
            .map(|key| crate::m2::press::canonical_key_name(key).ok())
            .collect();
        let reveal_text_key = keys.len() <= MAX_AUDIT_KEYS && Self::is_valid_shortcut_chord(&names);
        names
            .into_iter()
            .map(|name| match name {
                None => REDACTED_UNRECOGNIZED_KEY.to_owned(),
                Some(name)
                    if crate::m2::press::is_text_producing_key(&name) && !reveal_text_key =>
                {
                    REDACTED_PRINTABLE_KEY.to_owned()
                }
                Some(name) => bounded(&name, MAX_AUDIT_LABEL_CHARS),
            })
            .collect()
    }

    fn is_valid_shortcut_chord(names: &[Option<String>]) -> bool {
        let Some(recognized) = names.iter().cloned().collect::<Option<Vec<String>>>() else {
            return false;
        };
        let distinct: std::collections::HashSet<&str> =
            recognized.iter().map(String::as_str).collect();
        let text_keys = recognized
            .iter()
            .filter(|name| crate::m2::press::is_text_producing_key(name))
            .count();
        let has = |modifier: &str| distinct.contains(modifier);
        distinct.len() == recognized.len()
            && text_keys == 1
            && (has("super") || (has("ctrl") != has("alt")))
    }
}

fn bounded_id(id: &impl ToString) -> String {
    bounded(&id.to_string(), MAX_AUDIT_ID_CHARS)
}

fn bounded(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_owned()
    } else {
        let mut truncated: String = value.chars().take(max_chars).collect();
        truncated.push_str("...[truncated]");
        truncated
    }
}

fn debug_label(value: &impl std::fmt::Debug) -> String {
    format!("{value:?}")
}

#[cfg(test)]
#[path = "tests/act_request_audit.rs"]
mod act_request_audit_tests;
