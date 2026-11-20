use super::*;

pub(crate) fn element_coordinate_fallback_metadata(
    element_id: &synapse_core::ElementId,
) -> Result<synapse_a11y::ElementMetadataReadback, ErrorData> {
    synapse_a11y::element_metadata(element_id)
        .map_err(|error| action_error_to_mcp(&element_resolution_error(element_id, error)))
}

pub(crate) fn coordinate_fallback_allowed_for_metadata(
    metadata: &synapse_a11y::ElementMetadataReadback,
) -> bool {
    metadata.enabled
        && metadata.keyboard_focusable
        && metadata.bbox.w > 0
        && metadata.bbox.h > 0
        && (editable_role(&metadata.role) || exposes_text_value_pattern(&metadata.patterns))
}

/// Metadata of a target whose coordinate click needs real foreground caret
/// placement; `None` when the background tier may be used.
pub(crate) fn coordinate_click_foreground_requirement(
    element_id: &synapse_core::ElementId,
) -> Result<Option<synapse_a11y::ElementMetadataReadback>, ErrorData> {
    if element_is_coordinate_only(element_id) {
        return Ok(None);
    }
    let metadata = element_coordinate_fallback_metadata(element_id)?;
    Ok(coordinate_fallback_requires_foreground_caret_placement(&metadata).then_some(metadata))
}

/// #107: which tier a coordinate click by element id uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoordinateTier {
    /// WM_* mouse messages to the child window under the element; no
    /// foreground, no cursor.
    PostMessage,
    /// Leased SendInput on the OS foreground.
    Foreground,
}

/// #107: the tier of a coordinate click by element id. A caret-type control
/// needs the foreground tier only while this session's claimed target IS the
/// OS foreground (#189/#194); when it is not (another window in front, or no
/// foreground at all), the click is posted to the control's child window.
/// The profile route (no claim) keeps the foreground tier.
pub(crate) const fn coordinate_tier(
    caret_required: bool,
    claimed_root: Option<i64>,
    foreground_root: Option<i64>,
) -> CoordinateTier {
    if !caret_required {
        return CoordinateTier::PostMessage;
    }
    match (claimed_root, foreground_root) {
        (Some(root), Some(foreground)) if root == foreground => CoordinateTier::Foreground,
        (Some(_), _) => CoordinateTier::PostMessage,
        (None, _) => CoordinateTier::Foreground,
    }
}

pub(crate) fn coordinate_fallback_requires_foreground_caret_placement(
    metadata: &synapse_a11y::ElementMetadataReadback,
) -> bool {
    coordinate_fallback_allowed_for_metadata(metadata)
}

pub(crate) fn coordinate_fallback_required_for_selection_only_list_item(
    metadata: &synapse_a11y::ElementMetadataReadback,
) -> bool {
    metadata.enabled && metadata.bbox.w > 0 && metadata.bbox.h > 0 && list_item_role(&metadata.role)
}

pub(crate) fn editable_role(role: &str) -> bool {
    let role = role.to_ascii_lowercase();
    role.contains("edit") || role.contains("document") || role.contains("text")
}

pub(crate) fn list_item_role(role: &str) -> bool {
    role.to_ascii_lowercase().contains("list item")
}

pub(crate) fn exposes_text_value_pattern(patterns: &[UiaPattern]) -> bool {
    patterns
        .iter()
        .any(|pattern| matches!(pattern, UiaPattern::Value | UiaPattern::Text))
}

pub(crate) fn selection_only_list_item_error(
    element_id: &synapse_core::ElementId,
    detail: &str,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!("action target invalid: {detail} for element {element_id}"),
        Some(json!({
            "code": error_codes::ACTION_TARGET_INVALID,
            "detail_code": "UIA_SELECTION_ITEM_IS_NOT_CLICK_ACTIVATION",
            "element_id": element_id.as_str(),
            "router_escalation_allowed": false,
            "router_stop_reason": "coordinate_fallback_on_unsupported_false",
            "recommended_pattern": "retry with coordinate_fallback_on_unsupported=true or click the element bbox center so the target receives the real click/activation event",
        })),
    )
}

pub(crate) fn element_pattern_unsupported_coordinate_fallback_disabled_error(
    element_id: &synapse_core::ElementId,
    detail: &str,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "element target exposes no supported UIA click control pattern and coordinate fallback is disabled: {detail}"
        ),
        Some(json!({
            "code": error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED,
            "detail_code": "UIA_CONTROL_PATTERN_UNSUPPORTED",
            "fallback_attempted": false,
            "router_escalation_allowed": false,
            "router_stop_reason": "coordinate_fallback_on_unsupported_false",
            "element_id": element_id.to_string(),
            "source_of_truth": "live UI Automation control-pattern availability on the re-resolved element",
            "detail": detail,
        })),
    )
}
