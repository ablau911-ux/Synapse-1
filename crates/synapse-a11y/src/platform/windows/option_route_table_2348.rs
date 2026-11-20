//! FROZEN behaviour table of Gitea #2348 round 2 (FF-06: picking an option
//! natively), rows N1-N9 (`OptionRoute`). Written from the issue table before
//! the code. Coders never edit this file; a wrong row is reported to the
//! manager.
//! Place: crates/synapse-a11y/src/platform/windows/option_route_table_2348.rs
//! (registered by the `#[cfg(test)] #[path]` lines in option_route.rs).
//! Facts per case: the real Win32 class of the element's own window ("" when
//! it has none), its ExpandCollapse state (`None` = pattern not exposed), and
//! how many options UIA shows under it.
//! Every row carries a CONTROL case, so a stub that always gives the same
//! answer (any delivery, or any refusal) fails every row.
//! Row N9 keeps today's SelectionItem delivery for a closed list that shows
//! its options; the probe ruled out a generally owner-safe alternative.

use crate::{ExpandState, NativeOptionDelivery};

use super::OptionRoute;

use NativeOptionDelivery::{ComboBoxMessages, UiaExpandThenSelect, UiaSelectionItemPattern};

fn route(
    window_class: &str,
    expand_state: Option<ExpandState>,
    options_seen: usize,
) -> Result<NativeOptionDelivery, String> {
    OptionRoute::new(window_class, expand_state, options_seen).delivery()
}

/// The refusal text, or a failure naming what came instead.
fn refused(
    window_class: &str,
    expand_state: Option<ExpandState>,
    options_seen: usize,
) -> Result<String, String> {
    match route(window_class, expand_state, options_seen) {
        Err(text) => Ok(text),
        Ok(delivery) => Err(format!(
            "{window_class:?} {expand_state:?} {options_seen}: expected a refusal, got {delivery:?}"
        )),
    }
}

#[test]
fn n1_a_classic_combo_box_is_picked_through_its_own_list_messages() {
    // R3: WinForms DropDownList, closed, UIA shows no option and no pattern.
    assert_eq!(route("ComboBox", None, 0), Ok(ComboBoxMessages));
    // CONTROL: a list with no window of its own keeps the UIA route.
    assert_eq!(route("", None, 3), Ok(UiaSelectionItemPattern), "control");
}

#[test]
fn n2_a_classic_combo_box_is_never_opened_even_when_uia_shows_everything() {
    assert_eq!(
        route("ComboBox", Some(ExpandState::Collapsed), 3),
        Ok(ComboBoxMessages)
    );
    assert_eq!(
        route("ComboBox", Some(ExpandState::Collapsed), 0),
        Ok(ComboBoxMessages)
    );
    // CONTROL: the same facts without the classic class open the list.
    assert_eq!(
        route("", Some(ExpandState::Collapsed), 0),
        Ok(UiaExpandThenSelect),
        "control"
    );
}

#[test]
fn n3_the_class_compares_without_case_and_only_the_combo_box_itself_counts() {
    for class in ["COMBOBOX", "combobox", " ComboBox "] {
        assert_eq!(route(class, None, 0), Ok(ComboBoxMessages), "{class:?}");
    }
    // Neighbours: the drop-down's own list, the extended combo box and a list
    // box are not classic combo boxes.
    for class in ["ComboLBox", "ComboBoxEx32", "ListBox"] {
        assert_eq!(
            route(class, None, 3),
            Ok(UiaSelectionItemPattern),
            "{class:?}"
        );
    }
}

#[test]
fn n4_a_list_box_that_shows_its_options_is_picked_with_selection_item() {
    assert_eq!(route("ListBox", None, 3), Ok(UiaSelectionItemPattern));
    // CONTROL: the classic combo box beside it goes through its messages.
    assert_eq!(route("ComboBox", None, 3), Ok(ComboBoxMessages), "control");
}

#[test]
fn n5_a_list_that_is_already_open_is_picked_with_selection_item() {
    assert_eq!(
        route("", Some(ExpandState::Expanded), 3),
        Ok(UiaSelectionItemPattern)
    );
    assert_eq!(
        route("", Some(ExpandState::PartiallyExpanded), 3),
        Ok(UiaSelectionItemPattern)
    );
    // CONTROL: closed and empty is opened first.
    assert_eq!(
        route("", Some(ExpandState::Collapsed), 0),
        Ok(UiaExpandThenSelect),
        "control"
    );
}

#[test]
fn n6_a_closed_list_that_shows_no_option_is_opened_then_picked_then_closed() {
    assert_eq!(
        route("", Some(ExpandState::Collapsed), 0),
        Ok(UiaExpandThenSelect)
    );
    // CONTROL: an open list is not opened again.
    assert_eq!(
        route("", Some(ExpandState::Expanded), 3),
        Ok(UiaSelectionItemPattern),
        "control"
    );
}

#[test]
fn n7_no_options_and_no_way_to_open_is_refused_saying_so() -> Result<(), String> {
    let text = refused("", None, 0)?;
    assert!(text.contains("no options"), "{text}");
    assert!(text.contains("cannot be opened"), "{text}");
    // CONTROL: the same element with ExpandCollapse (closed) is opened.
    assert_eq!(
        route("", Some(ExpandState::Collapsed), 0),
        Ok(UiaExpandThenSelect),
        "control"
    );
    Ok(())
}

#[test]
fn n8_no_options_in_a_list_that_is_open_or_a_leaf_is_refused() -> Result<(), String> {
    for state in [
        ExpandState::LeafNode,
        ExpandState::Expanded,
        ExpandState::PartiallyExpanded,
    ] {
        let text = refused("", Some(state), 0)?;
        assert!(text.contains("no options"), "{state:?}: {text}");
    }
    // CONTROL: the open list with one option is picked.
    assert_eq!(
        route("", Some(ExpandState::Expanded), 1),
        Ok(UiaSelectionItemPattern),
        "control"
    );
    Ok(())
}

#[test]
fn n9_a_closed_list_that_shows_its_options_keeps_selection_item() {
    assert_eq!(
        route("", Some(ExpandState::Collapsed), 3),
        Ok(UiaSelectionItemPattern)
    );
    // CONTROL: a closed list with no option is opened first.
    assert_eq!(
        route("", Some(ExpandState::Collapsed), 0),
        Ok(UiaExpandThenSelect),
        "control"
    );
}
