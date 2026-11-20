//! FROZEN behaviour table of Gitea #2365 part b, rows K1-K6 (`DropdownKind`)
//! and V1-V3 (`ComboBoxReadback`). Written from the issue table before the
//! code. Coders never edit this file; a wrong row is reported to the manager.
//! Place: crates/synapse-a11y/src/dropdown_kind_table_2365b.rs (registered by
//! the second `#[path]` line at the top of `field_kind.rs`). One test per row,
//! named after the row. Extra control cases (marked CONTROL) stop a stub that
//! refuses everything, or gives one fixed answer, from passing a row.

use super::{ComboBoxReadback, Dropdown, DropdownKind};

/// The refusal text, or a failure naming what came instead.
fn refusal(kind: &DropdownKind) -> Result<String, String> {
    match kind.kind() {
        Dropdown::Refused(text) => Ok(text),
        other => Err(format!("{kind:?}: expected a refusal, got {other:?}")),
    }
}

#[test]
fn k1_select_is_a_browser_dropdown() {
    assert_eq!(
        DropdownKind::new("select", None, false).kind(),
        Dropdown::BrowserSelect
    );
}

#[test]
fn k2_role_combobox_is_a_custom_combo_box() {
    assert_eq!(
        DropdownKind::new("button", Some("combobox"), false).kind(),
        Dropdown::CustomComboBox,
        "button role=combobox"
    );
    // The div of the row also has aria-haspopup="listbox"; the role alone decides.
    assert_eq!(
        DropdownKind::new("div", Some("combobox"), false).kind(),
        Dropdown::CustomComboBox,
        "div role=combobox aria-haspopup=listbox"
    );
}

#[test]
fn k3_no_combobox_role_keeps_todays_refusal() -> Result<(), String> {
    let text = refusal(&DropdownKind::new("button", None, false))?;
    assert!(
        text.contains("the <button> element is not a dropdown (<select>)"),
        "{text}"
    );
    let text = refusal(&DropdownKind::new("div", Some("button"), false))?;
    assert!(
        text.contains("the <div> element is not a dropdown (<select>)"),
        "{text}"
    );
    // CONTROL: the same button with role=combobox is not refused.
    assert_eq!(
        DropdownKind::new("button", Some("combobox"), false).kind(),
        Dropdown::CustomComboBox
    );
    Ok(())
}

#[test]
fn k4_input_combobox_is_refused_pointing_to_set_field_and_click() -> Result<(), String> {
    let text = refusal(&DropdownKind::new("input", Some("combobox"), false))?;
    for word in ["set_field", "click"] {
        assert!(text.contains(word), "{text}");
    }
    // CONTROL: not today's "not a dropdown" text.
    assert!(!text.contains("not a dropdown"), "{text}");
    Ok(())
}

#[test]
fn k5_disabled_custom_combo_box_is_refused_disabled() -> Result<(), String> {
    // `disabled` and `aria-disabled="true"` both arrive as `disabled = true`.
    for tag in ["button", "div"] {
        let text = refusal(&DropdownKind::new(tag, Some("combobox"), true))?;
        assert!(text.contains("disabled"), "{tag}: {text}");
    }
    // CONTROL: the enabled one is picked.
    assert_eq!(
        DropdownKind::new("div", Some("combobox"), false).kind(),
        Dropdown::CustomComboBox
    );
    Ok(())
}

#[test]
fn k6_capitals_read_as_lower_case() -> Result<(), String> {
    let lower = DropdownKind::new("button", Some("combobox"), false).kind();
    // CONTROL: the lower-case answer is the row K2 answer, not a fixed one.
    assert_eq!(lower, Dropdown::CustomComboBox);
    assert_eq!(
        DropdownKind::new("BUTTON", Some("ComboBox"), false).kind(),
        lower
    );
    assert_eq!(
        DropdownKind::new("SELECT", None, false).kind(),
        Dropdown::BrowserSelect
    );
    let text = refusal(&DropdownKind::new("BUTTON", None, false))?;
    assert!(
        text.contains("the <button> element is not a dropdown (<select>)"),
        "{text}"
    );
    Ok(())
}

#[test]
fn v1_trigger_showing_the_option_is_verified() {
    assert_eq!(ComboBoxReadback::new("Rent", "Rent").verdict(), Ok(()));
    assert_eq!(ComboBoxReadback::new("Rent", " rent ").verdict(), Ok(()));
}

#[test]
fn v2_unchanged_trigger_is_not_confirmed_naming_both_texts() -> Result<(), String> {
    let Err(text) = ComboBoxReadback::new("Rent", "Category").verdict() else {
        return Err("a trigger still showing Category was verified".to_owned());
    };
    for part in ["Rent", "Category"] {
        assert!(text.contains(part), "{text}");
    }
    Ok(())
}

#[test]
fn v3_only_the_exact_text_counts() {
    assert!(
        ComboBoxReadback::new("Rent", "Rent (monthly)")
            .verdict()
            .is_err(),
        "a longer text was verified"
    );
    // CONTROL: the exact text is verified (a stub that refuses everything fails here).
    assert_eq!(ComboBoxReadback::new("Rent", "Rent").verdict(), Ok(()));
}
