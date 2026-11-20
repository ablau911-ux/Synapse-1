use super::*;

#[derive(Default)]
pub(crate) struct ElementClickTraceReadback<'a> {
    pub(crate) fallback_screen_point: Option<Point>,
    pub(crate) state_before: Option<&'a str>,
    pub(crate) state_after: Option<&'a str>,
    pub(crate) selected_before: Option<bool>,
    pub(crate) selected_after: Option<bool>,
    pub(crate) legacy_default_action: Option<&'a str>,
}

pub(crate) fn trace_element_click_outcome(
    element: &ActClickElementTarget,
    click_index: u8,
    outcome: &'static str,
    readback: ElementClickTraceReadback<'_>,
) {
    tracing::info!(
        code = "M2_ACT_CLICK_ELEMENT_READBACK",
        kind = "act_click",
        element_id = %element.element_id,
        click_number = u32::from(click_index) + 1,
        outcome,
        state_before = readback.state_before,
        state_after = readback.state_after,
        selected_before = readback.selected_before,
        selected_after = readback.selected_after,
        legacy_default_action = readback.legacy_default_action,
        fallback_screen_x = readback.fallback_screen_point.map(|point| point.x),
        fallback_screen_y = readback.fallback_screen_point.map(|point| point.y),
        "readback=action_backend tool=act_click element_click_after"
    );
}

pub(crate) fn trace_semantic_outcome(
    element: &ActClickElementTarget,
    index: u8,
    name: &'static str,
    outcome: &ElementClickOutcome,
) {
    let (before, after) = match outcome {
        ElementClickOutcome::Toggled {
            before_state,
            after_state,
        } => (Some(before_state.clone()), Some(after_state.clone())),
        ElementClickOutcome::Expanded {
            before_state,
            after_state,
        }
        | ElementClickOutcome::Collapsed {
            before_state,
            after_state,
        } => (
            Some(format!("{before_state:?}")),
            Some(format!("{after_state:?}")),
        ),
        _ => (None, None),
    };
    let mut readback = ElementClickTraceReadback {
        state_before: before.as_deref(),
        state_after: after.as_deref(),
        ..ElementClickTraceReadback::default()
    };
    if let ElementClickOutcome::Selected {
        was_selected,
        is_selected,
    } = outcome
    {
        readback.selected_before = Some(*was_selected);
        readback.selected_after = Some(*is_selected);
    }
    if let ElementClickOutcome::LegacyDefaultAction { default_action } = outcome {
        readback.legacy_default_action = default_action.as_deref();
    }
    trace_element_click_outcome(element, index, name, readback);
}

/// Pre-dispatch route decisions taken from live element metadata.
fn trace_metadata_route(
    code: &'static str,
    allowed: bool,
    element: &ActClickElementTarget,
    metadata: &synapse_a11y::ElementMetadataReadback,
    message: &'static str,
) {
    if allowed {
        tracing::info!(
            code,
            kind = "act_click",
            element_id = %element.element_id,
            role = %metadata.role,
            automation_id = metadata.automation_id.as_deref(),
            enabled = metadata.enabled,
            keyboard_focusable = metadata.keyboard_focusable,
            patterns = ?metadata.patterns,
            bbox = ?metadata.bbox,
            "{message}"
        );
    } else {
        tracing::warn!(
            code,
            kind = "act_click",
            element_id = %element.element_id,
            role = %metadata.role,
            automation_id = metadata.automation_id.as_deref(),
            enabled = metadata.enabled,
            keyboard_focusable = metadata.keyboard_focusable,
            patterns = ?metadata.patterns,
            bbox = ?metadata.bbox,
            "{message}"
        );
    }
}

pub(crate) fn trace_selection_only_route(
    element: &ActClickElementTarget,
    metadata: &synapse_a11y::ElementMetadataReadback,
    allowed: bool,
) {
    if allowed {
        trace_metadata_route(
            "M2_ACT_CLICK_COORDINATE_FALLBACK_ON_SELECTION_ONLY_LIST_ITEM",
            true,
            element,
            metadata,
            "act_click list item target requires click activation semantics; routing around selection-only UIA path",
        );
    } else {
        trace_metadata_route(
            "M2_ACT_CLICK_SELECTION_ONLY_LIST_ITEM_DENIED",
            false,
            element,
            metadata,
            "act_click list item target requires click activation semantics but coordinate fallback is disabled",
        );
    }
}

pub(crate) fn trace_unsupported_route(
    element: &ActClickElementTarget,
    metadata: &synapse_a11y::ElementMetadataReadback,
    allowed: bool,
) {
    if allowed {
        trace_metadata_route(
            "M2_ACT_CLICK_COORDINATE_FALLBACK_ON_UNSUPPORTED",
            true,
            element,
            metadata,
            "act_click UIA pattern unsupported; explicit coordinate fallback allowed for focusable edit/document-like element",
        );
    } else {
        trace_metadata_route(
            "M2_ACT_CLICK_COORDINATE_FALLBACK_DENIED",
            false,
            element,
            metadata,
            "act_click UIA pattern unsupported; coordinate fallback denied because element is not an enabled focusable edit/document-like target",
        );
    }
}

pub(crate) fn trace_caret_route(
    element: &ActClickElementTarget,
    metadata: &synapse_a11y::ElementMetadataReadback,
) {
    tracing::info!(
        code = "M2_ACT_CLICK_FOREGROUND_CARET_ROUTE",
        kind = "act_click",
        element_id = %element.element_id,
        role = %metadata.role,
        automation_id = metadata.automation_id.as_deref(),
        patterns = ?metadata.patterns,
        reason = "caret_placement_requires_real_foreground_click",
        "act_click coordinate element target requires real foreground input for caret placement; skipping HWND PostMessage"
    );
}

pub(crate) fn trace_postmessage_before_foreground(params: &ActClickParams, error: &ErrorData) {
    let element_id = match &params.target {
        super::super::ActClickTarget::Element(element) => Some(element.element_id.to_string()),
        super::super::ActClickTarget::Point(_) => None,
    };
    tracing::warn!(
        code = "M2_ACT_CLICK_POSTMESSAGE_BEFORE_FOREGROUND_FAILED",
        kind = "act_click",
        element_id = element_id.as_deref(),
        error_code = %click_error_code(error),
        "act_click element coordinate route could not use the background HWND message tier; escalating to leased foreground input"
    );
}
