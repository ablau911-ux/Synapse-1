use uiautomation::types::ToggleState;

use super::*;

#[path = "state_settle.rs"]
mod state_settle;

use state_settle::StateSettle;

#[path = "option_pick.rs"]
mod option_pick;
pub use option_pick::select_element_option;

pub fn invoke_element_pattern(id: &ElementId) -> A11yResult<()> {
    let id = id.clone();
    with_automation(move |automation| {
        let element = re_resolve_on_worker(automation, &id)?;
        let pattern: UIInvokePattern =
            element
                .get_pattern()
                .map_err(|error| A11yError::ElementPatternUnsupported {
                    detail: format!("InvokePattern not exposed for element {id}: {error}"),
                })?;
        invoke_pattern_action(&id, &pattern)
    })
    .map_err(click_dispatch_evidence)
}

pub fn set_element_checked(id: &ElementId, checked: bool) -> A11yResult<(String, String)> {
    let id = id.clone();
    let expected = if checked {
        ToggleState::On
    } else {
        ToggleState::Off
    };
    with_automation(move |automation| {
        let element = re_resolve_on_worker(automation, &id)?;
        let pattern: UITogglePattern =
            element
                .get_pattern()
                .map_err(|error| A11yError::ElementPatternUnsupported {
                    detail: format!("TogglePattern not exposed for element {id}: {error}"),
                })?;
        let before = pattern.get_toggle_state().map_err(|error| {
            pattern_operation_error(&id, "TogglePattern", "ToggleState before Toggle", &error)
        })?;
        let mut after = before;
        if after != expected {
            after = toggle_and_read_state(&id, &pattern, before)?;
            // A three-state provider may pass through Indeterminate.
            if after != expected {
                after = toggle_and_read_state(&id, &pattern, after).map_err(|source| {
                    A11yError::ClickDispatch {
                        source: Box::new(source),
                        delivered: true,
                    }
                })?;
            }
            if after != expected {
                return Err(A11yError::ClickDispatch {
                    source: Box::new(A11yError::internal(format!(
                        "TogglePattern for element {id} reached {after:?}, expected {expected:?}"
                    ))),
                    delivered: true,
                });
            }
        }
        Ok((toggle_state_label(before), toggle_state_label(after)))
    })
    .map_err(click_dispatch_evidence)
}

pub fn click_element_action(id: &ElementId) -> A11yResult<ElementClickAction> {
    let id = id.clone();
    with_automation(move |automation| click_on_worker(automation, &id))
        .map_err(click_dispatch_evidence)
}

/// #189 R5: every click failure leaves this producer typed by dispatch stage.
/// Pattern methods are wrapped by [`click_dispatch_with`]; a job the worker
/// received but never answered may have dispatched; every other failure
/// (admission, send, re-resolve, before-state reads, unsupported) precedes any
/// pattern call.
fn click_dispatch_evidence(error: A11yError) -> A11yError {
    match error {
        A11yError::ClickDispatch { .. } | A11yError::ClickNotDispatched { .. } => error,
        A11yError::UiaWorkerTimeout { phase, .. } if phase.job_sent() => A11yError::ClickDispatch {
            source: Box::new(error),
            delivered: false,
        },
        A11yError::UiaWorkerReplyLost { .. } => A11yError::ClickDispatch {
            source: Box::new(error),
            delivered: false,
        },
        other => A11yError::ClickNotDispatched {
            source: Box::new(other),
        },
    }
}

fn click_on_worker(automation: &UIAutomation, id: &ElementId) -> A11yResult<ElementClickAction> {
    let element = re_resolve_on_worker(automation, id)?;
    let mut unsupported_reasons = Vec::new();
    for pattern in click_pattern_order(&element) {
        let action = match pattern {
            ClickPatternKind::Invoke => try_invoke_pattern(id, &element, &mut unsupported_reasons)?,
            ClickPatternKind::Toggle => try_toggle_pattern(id, &element, &mut unsupported_reasons)?,
            ClickPatternKind::SelectionItem => {
                try_selection_item_pattern(id, &element, &mut unsupported_reasons)?
            }
            ClickPatternKind::ExpandCollapse => {
                try_expand_collapse_pattern(id, &element, &mut unsupported_reasons)?
            }
            ClickPatternKind::LegacyDefaultAction => {
                try_legacy_default_action_pattern(id, &element, &mut unsupported_reasons)?
            }
        };
        if let Some(action) = action {
            return Ok(action);
        }
    }
    Err(unsupported_click_pattern(
        id,
        &element,
        &unsupported_reasons,
    ))
}

fn click_pattern_order(element: &UIElement) -> [ClickPatternKind; 5] {
    use ClickPatternKind::{ExpandCollapse, Invoke, LegacyDefaultAction, SelectionItem, Toggle};

    let role = cached_role(element).to_ascii_lowercase();
    if role.contains("check") || role.contains("toggle") {
        return [
            Toggle,
            Invoke,
            SelectionItem,
            ExpandCollapse,
            LegacyDefaultAction,
        ];
    }
    if role.contains("list item") || role.contains("radio") || role.contains("tab item") {
        return [
            SelectionItem,
            Invoke,
            Toggle,
            ExpandCollapse,
            LegacyDefaultAction,
        ];
    }
    if role.contains("combo")
        || role.contains("menu item")
        || role.contains("tree item")
        || role.contains("split button")
    {
        return [
            ExpandCollapse,
            Invoke,
            Toggle,
            SelectionItem,
            LegacyDefaultAction,
        ];
    }
    [
        Invoke,
        Toggle,
        SelectionItem,
        ExpandCollapse,
        LegacyDefaultAction,
    ]
}

fn try_invoke_pattern(
    id: &ElementId,
    element: &UIElement,
    unsupported_reasons: &mut Vec<String>,
) -> A11yResult<Option<ElementClickAction>> {
    let invoke_pattern: Result<UIInvokePattern, _> = element.get_pattern();
    match invoke_pattern {
        Ok(invoke_pattern) => {
            invoke_pattern_action(id, &invoke_pattern)?;
            Ok(Some(ElementClickAction::Invoked))
        }
        Err(err) => {
            unsupported_reasons.push(format!("InvokePattern not exposed: {err}"));
            Ok(None)
        }
    }
}

fn invoke_pattern_action(id: &ElementId, pattern: &UIInvokePattern) -> A11yResult<()> {
    click_dispatch_with(
        || {
            pattern
                .invoke()
                .map_err(|err| pattern_operation_error(id, "InvokePattern", "invoke", &err))
        },
        || Ok(()),
    )
}

fn try_toggle_pattern(
    id: &ElementId,
    element: &UIElement,
    unsupported_reasons: &mut Vec<String>,
) -> A11yResult<Option<ElementClickAction>> {
    let toggle_pattern: Result<UITogglePattern, _> = element.get_pattern();
    match toggle_pattern {
        Ok(toggle_pattern) => toggle_pattern_action(id, &toggle_pattern).map(Some),
        Err(err) => {
            unsupported_reasons.push(format!("TogglePattern not exposed: {err}"));
            Ok(None)
        }
    }
}

fn toggle_pattern_action(
    id: &ElementId,
    toggle_pattern: &UITogglePattern,
) -> A11yResult<ElementClickAction> {
    let before = toggle_pattern.get_toggle_state().map_err(|err| {
        pattern_operation_error(id, "TogglePattern", "ToggleState before Toggle", &err)
    })?;
    let after = toggle_and_read_state(id, toggle_pattern, before)?;
    Ok(ElementClickAction::Toggled {
        before_state: toggle_state_label(before),
        after_state: toggle_state_label(after),
    })
}

fn toggle_and_read_state(
    id: &ElementId,
    toggle_pattern: &UITogglePattern,
    before: ToggleState,
) -> A11yResult<ToggleState> {
    click_dispatch_with(
        || {
            toggle_pattern
                .toggle()
                .map_err(|err| pattern_operation_error(id, "TogglePattern", "toggle", &err))
        },
        || {
            // #2349: Firefox updates ToggleState a moment after Toggle returns.
            let after = StateSettle::new(
                before,
                || {
                    toggle_pattern.get_toggle_state().map_err(|err| {
                        pattern_operation_error(
                            id,
                            "TogglePattern",
                            "ToggleState after Toggle",
                            &err,
                        )
                    })
                },
                std::thread::sleep,
            )
            .until_changed()?;
            if before == after {
                return Err(A11yError::internal(format!(
                    "TogglePattern.toggle returned for element {id}, but ToggleState stayed {after:?}"
                )));
            }
            Ok(after)
        },
    )
}

fn try_selection_item_pattern(
    id: &ElementId,
    element: &UIElement,
    unsupported_reasons: &mut Vec<String>,
) -> A11yResult<Option<ElementClickAction>> {
    let selection_item_pattern: Result<UISelectionItemPattern, _> = element.get_pattern();
    match selection_item_pattern {
        Ok(selection_item_pattern) => {
            selection_item_pattern_action(id, &selection_item_pattern).map(Some)
        }
        Err(err) => {
            unsupported_reasons.push(format!("SelectionItemPattern not exposed: {err}"));
            Ok(None)
        }
    }
}

fn selection_item_pattern_action(
    id: &ElementId,
    selection_item_pattern: &UISelectionItemPattern,
) -> A11yResult<ElementClickAction> {
    let was_selected = selection_item_pattern.is_selected().map_err(|err| {
        pattern_operation_error(id, "SelectionItemPattern", "IsSelected before Select", &err)
    })?;
    click_dispatch_with(
        || {
            selection_item_pattern
                .select()
                .map_err(|err| pattern_operation_error(id, "SelectionItemPattern", "select", &err))
        },
        || {
            // #2349: read until IsSelected leaves false; an item that was
            // already selected answers at the first read.
            let is_selected = StateSettle::new(
                false,
                || {
                    selection_item_pattern.is_selected().map_err(|err| {
                        pattern_operation_error(
                            id,
                            "SelectionItemPattern",
                            "IsSelected after Select",
                            &err,
                        )
                    })
                },
                std::thread::sleep,
            )
            .until_changed()?;
            if !is_selected {
                return Err(A11yError::internal(format!(
                    "SelectionItemPattern.select returned for element {id}, but IsSelected stayed false"
                )));
            }
            Ok(ElementClickAction::Selected {
                was_selected,
                is_selected,
            })
        },
    )
}

fn try_expand_collapse_pattern(
    id: &ElementId,
    element: &UIElement,
    unsupported_reasons: &mut Vec<String>,
) -> A11yResult<Option<ElementClickAction>> {
    let expand_collapse_pattern: Result<UIExpandCollapsePattern, _> = element.get_pattern();
    match expand_collapse_pattern {
        Ok(expand_collapse_pattern) => {
            expand_collapse_pattern_action(id, &expand_collapse_pattern, unsupported_reasons)
        }
        Err(err) => {
            unsupported_reasons.push(format!("ExpandCollapsePattern not exposed: {err}"));
            Ok(None)
        }
    }
}

fn expand_collapse_pattern_action(
    id: &ElementId,
    expand_collapse_pattern: &UIExpandCollapsePattern,
    unsupported_reasons: &mut Vec<String>,
) -> A11yResult<Option<ElementClickAction>> {
    let before_state =
        expand_state_from_uia(expand_collapse_pattern.get_state().map_err(|err| {
            pattern_operation_error(
                id,
                "ExpandCollapsePattern",
                "ExpandCollapseState before action",
                &err,
            )
        })?);
    match before_state {
        ExpandState::Collapsed => expand_pattern_action(id, expand_collapse_pattern, before_state),
        ExpandState::Expanded | ExpandState::PartiallyExpanded => {
            collapse_pattern_action(id, expand_collapse_pattern, before_state)
        }
        ExpandState::LeafNode => {
            unsupported_reasons
                .push("ExpandCollapsePattern exposed but current state is LeafNode".to_owned());
            Ok(None)
        }
    }
}

fn expand_pattern_action(
    id: &ElementId,
    expand_collapse_pattern: &UIExpandCollapsePattern,
    before_state: ExpandState,
) -> A11yResult<Option<ElementClickAction>> {
    click_dispatch_with(
        || {
            expand_collapse_pattern
                .expand()
                .map_err(|err| pattern_operation_error(id, "ExpandCollapsePattern", "expand", &err))
        },
        || {
            let after_state = StateSettle::new(
                before_state,
                || {
                    expand_collapse_pattern
                        .get_state()
                        .map(expand_state_from_uia)
                        .map_err(|err| {
                            pattern_operation_error(
                                id,
                                "ExpandCollapsePattern",
                                "ExpandCollapseState after expand",
                                &err,
                            )
                        })
                },
                std::thread::sleep,
            )
            .until_changed()?;
            if after_state == before_state {
                return Err(A11yError::internal(format!(
                    "ExpandCollapsePattern.expand returned for element {id}, but state stayed {after_state:?}"
                )));
            }
            Ok(Some(ElementClickAction::Expanded {
                before_state,
                after_state,
            }))
        },
    )
}

fn collapse_pattern_action(
    id: &ElementId,
    expand_collapse_pattern: &UIExpandCollapsePattern,
    before_state: ExpandState,
) -> A11yResult<Option<ElementClickAction>> {
    click_dispatch_with(
        || {
            expand_collapse_pattern.collapse().map_err(|err| {
                pattern_operation_error(id, "ExpandCollapsePattern", "collapse", &err)
            })
        },
        || {
            let after_state = StateSettle::new(
                before_state,
                || {
                    expand_collapse_pattern
                        .get_state()
                        .map(expand_state_from_uia)
                        .map_err(|err| {
                            pattern_operation_error(
                                id,
                                "ExpandCollapsePattern",
                                "ExpandCollapseState after collapse",
                                &err,
                            )
                        })
                },
                std::thread::sleep,
            )
            .until_changed()?;
            if after_state == before_state {
                return Err(A11yError::internal(format!(
                    "ExpandCollapsePattern.collapse returned for element {id}, but state stayed {after_state:?}"
                )));
            }
            Ok(Some(ElementClickAction::Collapsed {
                before_state,
                after_state,
            }))
        },
    )
}

fn try_legacy_default_action_pattern(
    id: &ElementId,
    element: &UIElement,
    unsupported_reasons: &mut Vec<String>,
) -> A11yResult<Option<ElementClickAction>> {
    let legacy_pattern: Result<UILegacyIAccessiblePattern, _> = element.get_pattern();
    match legacy_pattern {
        Ok(legacy_pattern) => legacy_default_action(id, &legacy_pattern, unsupported_reasons),
        Err(err) => {
            unsupported_reasons.push(format!("LegacyIAccessiblePattern not exposed: {err}"));
            Ok(None)
        }
    }
}

fn legacy_default_action(
    id: &ElementId,
    legacy_pattern: &UILegacyIAccessiblePattern,
    unsupported_reasons: &mut Vec<String>,
) -> A11yResult<Option<ElementClickAction>> {
    let default_action = match legacy_pattern.get_default_action() {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => {
            unsupported_reasons
                .push("LegacyIAccessiblePattern exposed but DefaultAction is empty".to_owned());
            return Ok(None);
        }
        Err(err) => {
            if uia_error_is_stale(&err) {
                return Err(pattern_operation_error(
                    id,
                    "LegacyIAccessiblePattern",
                    "DefaultAction read",
                    &err,
                ));
            }
            unsupported_reasons.push(format!(
                "LegacyIAccessiblePattern DefaultAction read failed: {err}"
            ));
            return Ok(None);
        }
    };
    click_dispatch_with(
        || {
            legacy_pattern.do_default_action().map_err(|err| {
                pattern_operation_error(id, "LegacyIAccessiblePattern", "DoDefaultAction", &err)
            })
        },
        || {
            Ok(Some(ElementClickAction::LegacyDefaultAction {
                default_action: Some(default_action),
            }))
        },
    )
}

/// The same boundary surrounds every UIA click pattern. An error inside the
/// provider is ambiguous; a failed readback after a returned operation is delivered.
fn click_dispatch_with<T>(
    dispatch: impl FnOnce() -> A11yResult<()>,
    readback: impl FnOnce() -> A11yResult<T>,
) -> A11yResult<T> {
    dispatch().map_err(|source| A11yError::ClickDispatch {
        source: Box::new(source),
        delivered: false,
    })?;
    readback().map_err(|source| A11yError::ClickDispatch {
        source: Box::new(source),
        delivered: true,
    })
}

#[cfg(test)]
#[path = "tests/click_dispatch_189.rs"]
mod click_dispatch_189;
