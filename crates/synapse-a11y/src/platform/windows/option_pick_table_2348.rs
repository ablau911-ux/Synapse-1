//! FROZEN behaviour table of Gitea #2348, rows O1-O7 (`OptionPick`).
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-a11y/src/platform/windows/option_pick_table_2348.rs
//! (registered by the `#[cfg(test)] #[path]` lines in option_pick.rs). The
//! options are given as (visible name, selected); no window is needed.
//! O7 has a CONTROL case: a stub that always answers "already selected"
//! would pass its own case.

use crate::OptionChoice;

use super::{OptionPick, PickedOption};

fn options(names: &[(&str, bool)]) -> Vec<(String, bool)> {
    names
        .iter()
        .map(|(name, selected)| ((*name).to_owned(), *selected))
        .collect()
}

/// A, B, C, none selected.
fn abc() -> Vec<(String, bool)> {
    options(&[("A", false), ("B", false), ("C", false)])
}

fn pick(name: &str, list: &[(String, bool)]) -> Result<PickedOption, String> {
    OptionPick::new(OptionChoice::new(Some(name), None)?).choose(list)
}

/// The refusal text, or a failure naming what came instead.
fn refused(name: &str, list: &[(String, bool)]) -> Result<String, String> {
    match pick(name, list) {
        Err(text) => Ok(text),
        Ok(picked) => Err(format!("{name:?}: expected a refusal, got {picked:?}")),
    }
}

#[test]
fn o1_the_named_option_is_chosen() {
    assert_eq!(pick("B", &abc()), Ok(PickedOption::Select(1)));
}

#[test]
fn o2_names_compare_trimmed_and_without_case() {
    for name in ["b", " B "] {
        assert_eq!(pick(name, &abc()), Ok(PickedOption::Select(1)), "{name:?}");
    }
}

#[test]
fn o3_a_missing_name_is_refused_listing_the_options() -> Result<(), String> {
    let text = refused("D", &abc())?;
    for name in ["A", "B", "C"] {
        assert!(text.contains(name), "missing {name}: {text}");
    }
    Ok(())
}

#[test]
fn o4_two_options_with_the_name_are_refused() -> Result<(), String> {
    let list = options(&[("A", false), ("B", false), ("B", false)]);
    let text = refused("B", &list)?;
    assert!(text.contains("2 options"), "{text}");
    Ok(())
}

#[test]
fn o5_an_element_without_options_is_refused() -> Result<(), String> {
    let text = refused("A", &[])?;
    assert!(text.contains("no options"), "{text}");
    Ok(())
}

#[test]
fn o6_a_long_list_shows_twenty_names_and_counts_the_rest() -> Result<(), String> {
    let names: Vec<String> = (1..=25).map(|n| format!("Opt{n:02}")).collect();
    let list: Vec<(String, bool)> = names.iter().map(|name| (name.clone(), false)).collect();
    let text = refused("Missing", &list)?;
    assert!(text.contains("Opt01"), "{text}");
    assert!(text.contains("Opt20"), "{text}");
    assert!(!text.contains("Opt21"), "{text}");
    assert!(text.contains("5 more"), "{text}");
    Ok(())
}

#[test]
fn o7_an_option_already_selected_is_success_and_nothing_is_sent() {
    let list = options(&[("A", false), ("B", true)]);
    assert_eq!(pick("B", &list), Ok(PickedOption::AlreadySelected(1)));
    // CONTROL: when A is the selected one, B is selected for real.
    let list = options(&[("A", true), ("B", false)]);
    assert_eq!(pick("B", &list), Ok(PickedOption::Select(1)), "control");
}
