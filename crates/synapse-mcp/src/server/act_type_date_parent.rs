//! `act_type` verification for native `<input type=date>` controls read over UIA only (#80).
//!
//! Chromium/WebView2 expose each date part (month/day/year) as its own
//! "spin button" UIA element. Typing a full date moves focus between those
//! parts, so the focused element differs between the before/after readbacks
//! even though the user-visible target (the date control) is unchanged. When
//! both focused elements are spin buttons under the same UIA parent, the
//! parent control (its value, or the combined segment values) is the Source of
//! Truth instead of the single focused segment.

use super::{
    ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH, ActTypeResponse, ActTypeTextReadback, ErrorData,
    json, postcondition_failed_error, postcondition_observed_delta, source_no_observed_delta_error,
    text_sha256,
};
use synapse_core::AccessibleNode;

pub(super) const ACT_TYPE_DATE_PARENT_SOURCE: &str = "uia_date_parent";
const ACT_TYPE_DATE_PARENT_INTEGRITY: &str = "verify_delta_text_readback:uia_date_parent";

/// UIA readback of the parent date control that owns the focused segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ActTypeDateParentReadback {
    pub(super) parent_id: String,
    pub(super) parent_value: Option<String>,
    pub(super) segment_values: Vec<String>,
}

impl ActTypeDateParentReadback {
    pub(super) fn from_live(live: synapse_a11y::FocusedParentReadback) -> Self {
        Self {
            parent_id: live.parent_id,
            parent_value: live.parent_value.filter(|value| !value.trim().is_empty()),
            segment_values: live.child_spin_values,
        }
    }

    /// Candidate date texts: the parent's own value and the segments joined as `a/b/c`.
    fn candidate_texts(&self) -> Vec<String> {
        let mut texts = Vec::new();
        if let Some(value) = &self.parent_value {
            texts.push(value.clone());
        }
        if !self.segment_values.is_empty() {
            texts.push(self.segment_values.join("/"));
        }
        texts
    }

    fn contains_typed_date(&self, emitted: &str) -> bool {
        self.candidate_texts()
            .iter()
            .any(|text| date_text_matches_typed(text, emitted))
    }
}

/// UIA "spin button" (localized control type) or its raw `Spinner` control type.
pub(super) fn is_spin_button(role: &str) -> bool {
    let normalized: String = role
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    normalized == "spinbutton" || normalized == "spinner"
}

/// Parent readback for the currently focused date segment. The live UIA
/// tree-walker parent is authoritative because the depth-limited observation
/// snapshot often omits the spin buttons' parent; the snapshot is only a fallback.
pub(super) fn focused_date_parent_readback(
    focused_element_id: &str,
    focused_role: &str,
    elements: &[AccessibleNode],
) -> Option<ActTypeDateParentReadback> {
    focused_date_parent_readback_with(
        focused_element_id,
        focused_role,
        elements,
        synapse_a11y::focused_spin_button_parent,
    )
}

pub(super) fn focused_date_parent_readback_with(
    focused_element_id: &str,
    focused_role: &str,
    elements: &[AccessibleNode],
    live_parent: impl FnOnce() -> synapse_a11y::A11yResult<Option<synapse_a11y::FocusedParentReadback>>,
) -> Option<ActTypeDateParentReadback> {
    if !is_spin_button(focused_role) {
        return None;
    }
    match live_parent() {
        Ok(Some(live)) => Some(ActTypeDateParentReadback::from_live(live)),
        Ok(None) => date_parent_readback(focused_element_id, elements),
        Err(error) => {
            tracing::warn!(
                code = error.code(),
                "live UIA parent lookup for the focused date segment failed; using snapshot parent"
            );
            date_parent_readback(focused_element_id, elements)
        }
    }
}

/// Builds the parent readback when the focused element is a date segment
/// (spin button) with a known UIA parent. `None` for any other focus target.
pub(super) fn date_parent_readback(
    focused_element_id: &str,
    elements: &[AccessibleNode],
) -> Option<ActTypeDateParentReadback> {
    let focused = elements
        .iter()
        .find(|node| node.element_id.as_str() == focused_element_id)?;
    if !is_spin_button(&focused.role) {
        return None;
    }
    let parent_id = focused.parent.as_ref()?;
    let parent_value = elements
        .iter()
        .find(|node| node.element_id == *parent_id)
        .and_then(|node| node.value.clone())
        .filter(|value| !value.trim().is_empty());
    let segment_values = elements
        .iter()
        .filter(|node| node.parent.as_ref() == Some(parent_id) && is_spin_button(&node.role))
        .filter_map(|node| node.value.clone())
        .collect();
    Some(ActTypeDateParentReadback {
        parent_id: parent_id.as_str().to_owned(),
        parent_value,
        segment_values,
    })
}

/// Both readbacks describe date segments under the same UIA parent.
pub(super) fn shared_date_parent<'a>(
    before: &'a ActTypeTextReadback,
    after: &'a ActTypeTextReadback,
) -> Option<(&'a ActTypeDateParentReadback, &'a ActTypeDateParentReadback)> {
    let before_parent = before.date_parent.as_ref()?;
    let after_parent = after.date_parent.as_ref()?;
    (before_parent.parent_id == after_parent.parent_id).then_some((before_parent, after_parent))
}

/// True when `text` renders the 8 typed digits (`mmddyyyy` or locale order) as a
/// date: separated parts compared numerically (`9/12/2026`), ISO (`2026-09-12`),
/// or a compact 8-digit run.
pub(super) fn date_text_matches_typed(text: &str, emitted: &str) -> bool {
    if emitted.len() != 8 || !emitted.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let parts: Vec<&str> = text
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .collect();
    let number = |part: &str| part.parse::<u32>().ok();
    let typed = [&emitted[..2], &emitted[2..4], &emitted[4..]].map(number);
    match parts.as_slice() {
        [first, second, third] => {
            let found = [number(first), number(second), number(third)];
            found.iter().all(Option::is_some)
                && (found == typed || found == [typed[2], typed[0], typed[1]])
        }
        [run] => *run == emitted || *run == format!("{}{}", &emitted[4..], &emitted[..4]),
        _ => false,
    }
}

/// Verifies `act_type` when focus moved between segments of one date control.
pub(super) fn verify_act_type_date_parent_response(
    mut response: ActTypeResponse,
    before: &ActTypeTextReadback,
    after: &ActTypeTextReadback,
    before_hash: String,
    after_hash: String,
    verify_timeout_ms: u32,
    emitted: &str,
) -> Result<ActTypeResponse, ErrorData> {
    let source =
        format!("{ACT_TYPE_FOREGROUND_TEXT_SOURCE_OF_TRUTH}:{ACT_TYPE_DATE_PARENT_SOURCE}");
    let (Some(before_parent), Some(after_parent)) = (&before.date_parent, &after.date_parent)
    else {
        return Err(postcondition_failed_error(
            "act_type",
            &source,
            "date parent control readback was unavailable",
            before_hash,
            after_hash,
            json!({ "before": before.signature, "after": after.signature }),
        ));
    };
    if before_parent == after_parent {
        return Err(source_no_observed_delta_error(
            "act_type",
            &source,
            verify_timeout_ms,
            before_hash,
            after_hash,
            json!({ "before": before.signature, "after": after.signature }),
        ));
    }
    if !after_parent.contains_typed_date(emitted) {
        return Err(postcondition_failed_error(
            "act_type",
            &source,
            "date control Source-of-Truth changed but did not contain the emitted date",
            before_hash,
            after_hash,
            json!({
                "expected_emitted_len": emitted.chars().count(),
                "expected_emitted_sha256": text_sha256(emitted),
                "before": before.signature,
                "after": after.signature,
            }),
        ));
    }
    response.postcondition = postcondition_observed_delta(
        "act_type",
        &source,
        before_hash,
        after_hash,
        "observed date control parent value changed and containing emitted date after delivery",
    );
    response.target_readback_required = false;
    response.target_text_integrity = ACT_TYPE_DATE_PARENT_INTEGRITY.to_owned();
    Ok(response)
}
