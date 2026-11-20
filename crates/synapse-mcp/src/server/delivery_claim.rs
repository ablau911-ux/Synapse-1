//! The one place that decides the public `delivery_state` word of a
//! `target_act` reply (#2340).

use serde_json::Value;

use super::background_router::{
    TARGET_ACT_STATUS_DEFERRED, TARGET_ACT_STATUS_OK, TARGET_ACT_STATUS_REFUSED,
    TARGET_ACT_STATUS_VERIFY_NEEDED,
};

#[cfg(test)]
#[path = "delivery_claim_table_569.rs"]
mod delivery_claim_table_569;

/// Verbs that put text into a control.
const TEXT_VERBS: [&str; 4] = ["type", "set_field", "insert_text", "append_text"];
/// Verbs that send keys.
const KEY_VERBS: [&str; 3] = ["key", "key_chord", "press"];
/// Evidence names that mean "the pixels at a known place changed".
const PIXEL_SOURCES: [&str; 3] = [
    "target_point_pixels",
    "target_window_pixels",
    "foreground_pixels",
];
/// Evidence names that mean "anything in the window changed".
const WINDOW_CHANGE_SOURCES: [&str; 2] = [
    "target_window_ui_or_pixels",
    "foreground_focused_ui_or_pixels",
];
/// Evidence name that means "another window appeared".
const RELATED_WINDOW_SOURCE: &str = "foreground_related_window";

/// What one `target_act` reply may claim about the app, decided from the
/// evidence the route recorded in its result.
pub(super) struct DeliveryClaim<'a> {
    verb: &'a str,
    delivered: bool,
    status: &'a str,
    evidence: Option<Evidence<'a>>,
}

impl<'a> DeliveryClaim<'a> {
    /// Reads the reply once and finds the evidence the route recorded.
    pub(super) fn new(verb: &'a str, ok: bool, status: &'a str, result: &'a Value) -> Self {
        Self {
            verb,
            delivered: ok && status == TARGET_ACT_STATUS_OK,
            status,
            evidence: Evidence::find(verb, result),
        }
    }

    /// A reply that carries no result (refusals and failures before delivery).
    pub(super) fn without_result(ok: bool, status: &'a str) -> Self {
        Self {
            verb: "",
            delivered: ok && status == TARGET_ACT_STATUS_OK,
            status,
            evidence: None,
        }
    }

    /// The public `delivery_state` word.
    pub(super) fn state(&self) -> &'static str {
        if !self.delivered {
            return match self.status {
                TARGET_ACT_STATUS_VERIFY_NEEDED => "delivered_unverified",
                TARGET_ACT_STATUS_REFUSED => "refused_before_delivery",
                TARGET_ACT_STATUS_DEFERRED => "deferred_without_delivery",
                _ => "failed_before_delivery",
            };
        }
        let Some(evidence) = &self.evidence else {
            return "delivered_unverified";
        };
        let source = evidence.source_of_truth;
        if evidence.pixels_changed && PIXEL_SOURCES.contains(&source) {
            return "delivered_and_pixel_verified";
        }
        // A change somewhere in the window proves that the app reacted, not what
        // it did: an error message is such a change (#2367). Text and key verbs
        // keep the weaker word for this evidence name in every case (#2340).
        let text_verb = TEXT_VERBS.contains(&self.verb);
        if WINDOW_CHANGE_SOURCES.contains(&source)
            && (evidence.observed || text_verb || KEY_VERBS.contains(&self.verb))
        {
            return "delivered_and_pixel_verified";
        }
        if text_verb && source == RELATED_WINDOW_SOURCE {
            return "delivered_unverified";
        }
        "delivered_and_verified"
    }
}

/// A confirming after-read that a route recorded: what was compared, and
/// whether two real pixel hashes differ.
struct Evidence<'a> {
    source_of_truth: &'a str,
    /// True for an observed change; false for a state that was read back.
    observed: bool,
    pixels_changed: bool,
}

impl<'a> Evidence<'a> {
    /// Finds the confirming evidence of this reply, wherever the route put it.
    fn find(verb: &str, result: &'a Value) -> Option<Self> {
        if let Some(postcondition) = result.get("postcondition") {
            return Self::from_postcondition(postcondition);
        }
        if !TEXT_VERBS.contains(&verb) {
            return None;
        }
        if let Some(postcondition) = result
            .get("type")
            .and_then(|typed| typed.get("postcondition"))
        {
            return Self::from_postcondition(postcondition);
        }
        if let Some(steps) = result.get("steps").and_then(Value::as_array) {
            return steps
                .iter()
                .rev()
                .find(|step| step.get("step").and_then(Value::as_str) == Some("type"))
                .and_then(|step| step.get("result"))
                .and_then(|step_result| step_result.get("postcondition"))
                .and_then(Self::from_postcondition);
        }
        if verb == "set_field"
            && result.get("status").and_then(Value::as_str) == Some("verified_state")
        {
            return Some(Self::exact(
                result
                    .get("source_of_truth")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ));
        }
        if matches!(verb, "insert_text" | "append_text") {
            let after = result.get("after_text_utf16_len").and_then(Value::as_u64);
            let expected = result
                .get("expected_after_text_utf16_len")
                .and_then(Value::as_u64);
            if after.is_some() && after == expected {
                return Some(Self::exact("win32_window_text"));
            }
        }
        None
    }

    /// Evidence of a route that compared the exact text itself.
    const fn exact(source_of_truth: &'a str) -> Self {
        Self {
            source_of_truth,
            observed: false,
            pixels_changed: false,
        }
    }

    /// `None` unless the postcondition confirms something.
    fn from_postcondition(postcondition: &'a Value) -> Option<Self> {
        let text = |key: &str| -> &'a str {
            postcondition
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let observed = match postcondition.get("status").and_then(Value::as_str) {
            Some("observed_delta") => {
                if postcondition.get("observed_delta").and_then(Value::as_bool) != Some(true) {
                    return None;
                }
                true
            }
            Some("verified_state") => false,
            _ => return None,
        };
        let (before, after) = (text("before_signature"), text("after_signature"));
        Some(Self {
            source_of_truth: text("source_of_truth"),
            observed,
            pixels_changed: observed
                && before != after
                && Self::is_pixel_hash(before)
                && Self::is_pixel_hash(after),
        })
    }

    /// A SHA-256 pixel hash as the capture code writes it: 64 hex digits.
    fn is_pixel_hash(signature: &str) -> bool {
        signature.len() == 64 && signature.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}
