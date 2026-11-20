//! #2348: `act select` on a native list or combo box. `OptionPick` picks ONE
//! option by the caller's choice (#2365 `OptionChoice`: visible name, value
//! or index) through the delivery `OptionRoute` chooses: a classic combo box
//! through its own list messages (`ComboList`, CB_GETCURSEL read back), any
//! other list through SelectionItemPattern with `IsSelected` read back, a
//! closed list that shows no option opened through ExpandCollapse first. The
//! choice from a list of (name, selected) is pure, so the table needs no
//! window; the UIA calls run on the crate's UIA worker.

use uiautomation::variants::Variant;
use windows::Win32::UI::WindowsAndMessaging::RealGetWindowClassW;

use super::*;
use crate::{NativeOptionDelivery, NativeOptionReadback, OptionChoice};
use combo_list::ComboList;
use option_route::OptionRoute;

#[cfg(test)]
#[path = "option_pick_table_2348.rs"]
mod option_pick_table_2348;

#[path = "option_route.rs"]
mod option_route;

#[path = "combo_list.rs"]
mod combo_list;

/// What one pick does with the options it read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickedOption {
    /// Select the option at this index (from 0).
    Select(usize),
    /// The option at this index is already selected: nothing is sent.
    AlreadySelected(usize),
}

/// One `act select` on a native element: the option the caller means.
pub(super) struct OptionPick {
    choice: OptionChoice,
}

impl OptionPick {
    pub(super) const fn new(choice: OptionChoice) -> Self {
        Self { choice }
    }

    /// The option to select among `options` ((visible name, selected), in
    /// tree order). Names compare as `OptionChoice::pick` compares labels
    /// (trimmed, without case).
    ///
    /// # Errors
    ///
    /// The refusal text: no options, no option with that name (listing at
    /// most 20 names and how many more exist), or two options with it.
    pub(super) fn choose(&self, options: &[(String, bool)]) -> Result<PickedOption, String> {
        let labels: Vec<(String, String)> = options
            .iter()
            .map(|(name, _)| (name.clone(), name.clone()))
            .collect();
        let index = self.choice.pick(&labels)?;
        if options.get(index).is_some_and(|(_, selected)| *selected) {
            Ok(PickedOption::AlreadySelected(index))
        } else {
            Ok(PickedOption::Select(index))
        }
    }

    /// On the UIA worker: read the facts of the element, let [`OptionRoute`]
    /// choose the delivery, then pick. A classic combo box goes through its
    /// own list messages ([`ComboList`]); a closed list that shows no option
    /// is opened, picked and closed again; any other list is picked through
    /// SelectionItemPattern with `IsSelected` read back.
    fn select_on_worker(
        &self,
        automation: &UIAutomation,
        id: &ElementId,
    ) -> A11yResult<NativeOptionReadback> {
        let element = re_resolve_on_worker(automation, id)?;
        let own_window = cached_hwnd(&element).unwrap_or(0);
        let expand_collapse = element.get_pattern::<UIExpandCollapsePattern>().ok();
        // A state the provider cannot read counts as no ExpandCollapse.
        let expand_state = expand_collapse
            .as_ref()
            .and_then(|pattern| pattern.get_state().ok())
            .map(expand_state_from_uia);
        let items = Self::option_items(automation, &element)?;
        let delivery = OptionRoute::new(&Self::window_class(own_window), expand_state, items.len())
            .delivery()
            .map_err(|detail| A11yError::OptionRefused {
                detail: format!("element {id}: {detail}"),
            })?;
        match delivery {
            NativeOptionDelivery::ComboBoxMessages => ComboList::new(id, own_window)?.pick(self),
            NativeOptionDelivery::UiaSelectionItemPattern => self.select_item(id, &items, delivery),
            NativeOptionDelivery::UiaExpandThenSelect => {
                let pattern = expand_collapse.ok_or_else(|| {
                    A11yError::internal(format!(
                        "ExpandCollapsePattern of element {id} vanished before expand"
                    ))
                })?;
                expand_pattern_action(id, &pattern, ExpandState::Collapsed)?;
                let picked = if items.is_empty() {
                    Self::wait_for_options(automation, &element)
                        .and_then(|opened| self.select_item(id, &opened, delivery))
                } else {
                    self.select_item(id, &items, delivery)
                };
                let closed = Self::close_again(id, &pattern);
                let readback = picked?;
                closed.map_err(|source| A11yError::ClickDispatch {
                    source: Box::new(source),
                    delivered: true,
                })?;
                Ok(readback)
            }
        }
    }

    /// SelectionItemPattern on the option `self` means among `items`, with
    /// `IsSelected` read back (an option already selected is not sent again).
    fn select_item(
        &self,
        id: &ElementId,
        items: &[UIElement],
        delivery: NativeOptionDelivery,
    ) -> A11yResult<NativeOptionReadback> {
        let mut options = Vec::with_capacity(items.len());
        for item in items {
            let selected = Self::selection_item(id, item)?
                .is_selected()
                .map_err(|err| {
                    pattern_operation_error(
                        id,
                        "SelectionItemPattern",
                        "IsSelected before Select",
                        &err,
                    )
                })?;
            options.push((cached_public_name(item), selected));
        }
        let picked = self
            .choose(&options)
            .map_err(|detail| A11yError::OptionRefused {
                detail: format!("element {id}: {detail}"),
            })?;
        let (index, was_selected, is_selected) = match picked {
            PickedOption::AlreadySelected(index) => (index, true, true),
            PickedOption::Select(index) => {
                let item = items.get(index).ok_or_else(|| {
                    A11yError::internal(format!(
                        "option {index} of element {id} vanished between read and select"
                    ))
                })?;
                match selection_item_pattern_action(id, &Self::selection_item(id, item)?)? {
                    ElementClickAction::Selected {
                        was_selected,
                        is_selected,
                    } => (index, was_selected, is_selected),
                    other => {
                        return Err(A11yError::internal(format!(
                            "SelectionItemPattern.select for an option of element {id} answered {other:?}"
                        )));
                    }
                }
            }
        };
        let name = options
            .get(index)
            .map(|(name, _)| name.clone())
            .unwrap_or_default();
        Ok(NativeOptionReadback::new(
            name,
            index,
            options.len(),
            was_selected,
            is_selected,
            delivery,
        ))
    }

    /// The options under `element`: every descendant that exposes
    /// SelectionItemPattern, in tree order.
    fn option_items(automation: &UIAutomation, element: &UIElement) -> A11yResult<Vec<UIElement>> {
        let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Control)?;
        let condition = automation
            .create_property_condition(
                UIProperty::IsSelectionItemPatternAvailable,
                Variant::from(true),
                None,
            )
            .map_err(map_uia_error)?;
        element
            .find_all_build_cache(TreeScope::Descendants, &condition, &cache)
            .map_err(map_uia_error)
    }

    fn selection_item(id: &ElementId, item: &UIElement) -> A11yResult<UISelectionItemPattern> {
        item.get_pattern()
            .map_err(|error| A11yError::ElementPatternUnsupported {
                detail: format!(
                    "SelectionItemPattern not exposed by an option of element {id}: {error}"
                ),
            })
    }

    /// The options of a list this pick just opened: read again until some
    /// show (at most the settle budget of #2349).
    fn wait_for_options(
        automation: &UIAutomation,
        element: &UIElement,
    ) -> A11yResult<Vec<UIElement>> {
        StateSettle::new(
            0_usize,
            || Self::option_items(automation, element).map(|found| found.len()),
            std::thread::sleep,
        )
        .until_changed()?;
        Self::option_items(automation, element)
    }

    /// Closes a list this pick opened, unless it closed itself.
    fn close_again(id: &ElementId, pattern: &UIExpandCollapsePattern) -> A11yResult<()> {
        let state = expand_state_from_uia(pattern.get_state().map_err(|err| {
            pattern_operation_error(
                id,
                "ExpandCollapsePattern",
                "ExpandCollapseState after the pick",
                &err,
            )
        })?);
        if state != ExpandState::Collapsed {
            collapse_pattern_action(id, pattern, state)?;
        }
        Ok(())
    }

    /// The real Win32 class of the element's own window (`RealGetWindowClassW`:
    /// a superclass, such as the WinForms combo box, reports its base class);
    /// empty when the element has no window of its own.
    fn window_class(own_window: i64) -> String {
        let Ok(raw) = isize::try_from(own_window) else {
            return String::new();
        };
        if raw == 0 {
            return String::new();
        }
        let mut name = [0_u16; 256];
        // SAFETY: the buffer is ours and its length goes with it.
        let len = unsafe { RealGetWindowClassW(HWND(raw as *mut c_void), &mut name) };
        String::from_utf16_lossy(name.get(..len as usize).unwrap_or_default())
    }
}

/// Selects one option of a native list or combo box (see [`OptionPick`]).
pub fn select_element_option(
    id: &ElementId,
    choice: &OptionChoice,
) -> A11yResult<NativeOptionReadback> {
    let id = id.clone();
    let pick = OptionPick::new(choice.clone());
    with_automation(move |automation| pick.select_on_worker(automation, &id))
        .map_err(click_dispatch_evidence)
}
