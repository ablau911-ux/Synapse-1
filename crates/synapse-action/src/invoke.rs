use synapse_core::{ElementId, MouseButton, Point};

#[cfg(not(windows))]
use crate::ActionError;
use crate::{ActionBackend, ActionResult, EmitState};

#[cfg(test)]
mod dispatch;
#[cfg(any(test, windows))]
mod resolver;

#[cfg(test)]
mod tests;

#[cfg(all(test, windows))]
#[path = "invoke/tests/click_dispatch_189.rs"]
mod click_dispatch_189;
#[cfg(all(test, windows))]
#[path = "invoke/tests/window_gone_mapping_315.rs"]
mod window_gone_mapping_315;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ElementClickOutcome {
    Invoked,
    Toggled {
        before_state: String,
        after_state: String,
    },
    Selected {
        was_selected: bool,
        is_selected: bool,
    },
    Expanded {
        before_state: synapse_a11y::ExpandState,
        after_state: synapse_a11y::ExpandState,
    },
    Collapsed {
        before_state: synapse_a11y::ExpandState,
        after_state: synapse_a11y::ExpandState,
    },
    LegacyDefaultAction {
        default_action: Option<String>,
    },
    CoordinateFallback(CoordinateFallbackPlan),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CoordinateFallbackPlan {
    pub screen_point: Point,
    pub window_point: Point,
}

/// Re-resolves a Synapse accessibility element and dispatches a semantic UIA
/// click without moving the cursor.
///
/// Supported patterns are `InvokePattern`, `TogglePattern`,
/// `SelectionItemPattern`, `ExpandCollapsePattern`, and
/// `LegacyIAccessiblePattern.DoDefaultAction`.
///
/// # Errors
///
/// On Windows, `synapse_a11y::re_resolve` failures are mapped to
/// `ACTION_ELEMENT_NOT_RESOLVED`. Missing semantic patterns are reported as
/// `ACTION_ELEMENT_PATTERN_UNSUPPORTED`; no coordinate click is attempted.
#[cfg(windows)]
pub fn invoke_element(element_id: &ElementId) -> ActionResult<()> {
    match synapse_a11y::click_element_action(element_id)
        .map_err(|error| a11y_error_to_action(element_id, error))?
    {
        synapse_a11y::ElementClickAction::Invoked
        | synapse_a11y::ElementClickAction::Toggled { .. }
        | synapse_a11y::ElementClickAction::Selected { .. }
        | synapse_a11y::ElementClickAction::Expanded { .. }
        | synapse_a11y::ElementClickAction::Collapsed { .. }
        | synapse_a11y::ElementClickAction::LegacyDefaultAction { .. } => Ok(()),
    }
}

/// Non-Windows builds expose the same API but fail closed before any action is
/// attempted.
///
/// # Errors
///
/// Always returns `ACTION_BACKEND_UNAVAILABLE` because UI Automation
/// `InvokePattern` dispatch is Windows-only.
#[cfg(not(windows))]
pub fn invoke_element(element_id: &ElementId) -> ActionResult<()> {
    Err(ActionError::BackendUnavailable {
        detail: format!("UI Automation InvokePattern requires Windows for element {element_id}"),
    })
}

/// Attempts a semantic UIA click using supported control patterns.
///
/// This path does not synthesize a coordinate fallback; unsupported patterns
/// fail with `ACTION_ELEMENT_PATTERN_UNSUPPORTED` so a higher-level router can
/// explicitly choose the next delivery tier.
///
/// # Errors
///
/// Returns `ACTION_ELEMENT_NOT_RESOLVED` when UIA re-resolution fails,
/// `ACTION_ELEMENT_PATTERN_UNSUPPORTED` when the element exposes no supported
/// click pattern, or a backend-specific action error if UIA dispatch fails.
#[cfg(windows)]
pub fn click_element_or_fallback<B>(
    element_id: &ElementId,
    backend: &B,
    state: &mut EmitState,
    button: MouseButton,
) -> ActionResult<ElementClickOutcome>
where
    B: ActionBackend,
{
    click_element_or_fallback_with(
        element_id,
        backend,
        state,
        button,
        synapse_a11y::click_element_action,
    )
}

/// [`click_element_or_fallback`] with an injected element-click resolver in
/// place of `synapse_a11y::click_element_action` (test seam). The resolver's
/// error is mapped exactly as on the live path; no coordinate fallback runs.
#[cfg(windows)]
pub(crate) fn click_element_or_fallback_with<B, R>(
    element_id: &ElementId,
    _backend: &B,
    _state: &mut EmitState,
    _button: MouseButton,
    click_element: R,
) -> ActionResult<ElementClickOutcome>
where
    B: ActionBackend,
    R: FnOnce(&ElementId) -> synapse_a11y::A11yResult<synapse_a11y::ElementClickAction>,
{
    let action =
        click_element(element_id).map_err(|error| a11y_error_to_action(element_id, error))?;
    Ok(element_click_outcome(action))
}

#[cfg(windows)]
fn element_click_outcome(action: synapse_a11y::ElementClickAction) -> ElementClickOutcome {
    match action {
        synapse_a11y::ElementClickAction::Invoked => ElementClickOutcome::Invoked,
        synapse_a11y::ElementClickAction::Toggled {
            before_state,
            after_state,
        } => ElementClickOutcome::Toggled {
            before_state,
            after_state,
        },
        synapse_a11y::ElementClickAction::Selected {
            was_selected,
            is_selected,
        } => ElementClickOutcome::Selected {
            was_selected,
            is_selected,
        },
        synapse_a11y::ElementClickAction::Expanded {
            before_state,
            after_state,
        } => ElementClickOutcome::Expanded {
            before_state,
            after_state,
        },
        synapse_a11y::ElementClickAction::Collapsed {
            before_state,
            after_state,
        } => ElementClickOutcome::Collapsed {
            before_state,
            after_state,
        },
        synapse_a11y::ElementClickAction::LegacyDefaultAction { default_action } => {
            ElementClickOutcome::LegacyDefaultAction { default_action }
        }
    }
}

/// Non-Windows builds expose the same API but fail closed before any action is
/// attempted.
///
/// # Errors
///
/// Always returns `ACTION_BACKEND_UNAVAILABLE` because UI Automation
/// `InvokePattern` dispatch and bounding-rectangle fallback are Windows-only.
#[cfg(not(windows))]
pub fn click_element_or_fallback<B>(
    element_id: &ElementId,
    _backend: &B,
    _state: &mut EmitState,
    _button: MouseButton,
) -> ActionResult<ElementClickOutcome>
where
    B: ActionBackend,
{
    // #189: refused before any dispatch; the UIA tier cannot run here.
    Err(ActionError::ClickNotDispatched {
        source: Box::new(ActionError::BackendUnavailable {
            detail: format!(
                "UI Automation element click requires Windows for element {element_id}"
            ),
        }),
        tier_unavailable: true,
    })
}

#[cfg(windows)]
#[expect(
    dead_code,
    reason = "reserved for element-target action paths that need screen-point readback"
)]
pub(crate) fn element_screen_point(element_id: &ElementId) -> ActionResult<Point> {
    let rect = synapse_a11y::element_bounding_rect(element_id)
        .map_err(|error| a11y_error_to_action(element_id, error))?;
    resolver::coordinate_fallback_plan(element_id, rect).map(|plan| plan.screen_point)
}

#[cfg(not(windows))]
#[expect(
    dead_code,
    reason = "reserved for element-target action paths that need screen-point readback"
)]
pub(crate) fn element_screen_point(element_id: &ElementId) -> ActionResult<Point> {
    Err(ActionError::BackendUnavailable {
        detail: format!("UI Automation element target requires Windows for element {element_id}"),
    })
}

#[cfg(windows)]
fn a11y_error_to_action(
    element_id: &ElementId,
    error: synapse_a11y::A11yError,
) -> crate::ActionError {
    let detail = error.to_string();
    if !matches!(
        &error,
        synapse_a11y::A11yError::ClickDispatch { .. }
            | synapse_a11y::A11yError::ClickNotDispatched { .. }
            | synapse_a11y::A11yError::WindowNotFound { .. }
    ) && stale_provider_failure(&detail)
    {
        return resolver::transient_element_expired(element_id, detail);
    }
    match error {
        synapse_a11y::A11yError::ClickDispatch { source, delivered } => {
            crate::ActionError::ClickDispatch {
                source: Box::new(a11y_error_to_action(element_id, *source)),
                delivered,
            }
        }
        synapse_a11y::A11yError::ClickNotDispatched { source } => {
            // #189 U9: only a worker admission refusal means the tier could not run.
            let tier_unavailable = matches!(
                &*source,
                synapse_a11y::A11yError::UiaWorkerTimeout { phase, .. } if !phase.job_sent()
            );
            crate::ActionError::ClickNotDispatched {
                source: Box::new(a11y_error_to_action(element_id, *source)),
                tier_unavailable,
            }
        }
        synapse_a11y::A11yError::ElementStale { .. } => {
            resolver::transient_element_expired(element_id, detail)
        }
        synapse_a11y::A11yError::ElementPatternUnsupported { .. }
        | synapse_a11y::A11yError::ElementValueUnsupported { .. } => {
            resolver::element_pattern_unsupported(element_id, detail)
        }
        synapse_a11y::A11yError::ElementValueReadOnly { .. }
        | synapse_a11y::A11yError::ElementNotEnabled { .. }
        | synapse_a11y::A11yError::OptionRefused { .. }
        | synapse_a11y::A11yError::CdpUnreachable { .. }
        | synapse_a11y::A11yError::CdpAttachFailed { .. }
        | synapse_a11y::A11yError::CdpAxtreeFailed { .. }
        | synapse_a11y::A11yError::BrowserWaitTimeout { .. }
        | synapse_a11y::A11yError::CdpStrokeTimeout { .. }
        | synapse_a11y::A11yError::UiaWorkerReplyLost { .. }
        | synapse_a11y::A11yError::Internal { .. } => resolver::target_invalid(detail),
        synapse_a11y::A11yError::WindowNotFound { detail } => {
            crate::ActionError::WindowNotFound { detail }
        }
        synapse_a11y::A11yError::InvalidElementId { .. }
        | synapse_a11y::A11yError::NoForeground { .. } => resolver::element_not_resolved(detail),
        synapse_a11y::A11yError::NotAvailable { detail } => {
            crate::ActionError::BackendUnavailable { detail }
        }
        synapse_a11y::A11yError::UiaWorkerTimeout { .. } => {
            crate::ActionError::BackendUnavailable { detail }
        }
        synapse_a11y::A11yError::ForegroundActivationRefused { .. } => {
            crate::ActionError::ForegroundActivationRefused { detail }
        }
    }
}

#[cfg(windows)]
fn stale_provider_failure(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("event was unable to invoke any of the subscribers")
        || detail.contains("element not available")
        || detail.contains("element is no longer available")
        || detail.contains("uia_e_elementnotavailable")
}
