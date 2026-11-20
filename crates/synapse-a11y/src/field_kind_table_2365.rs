//! FROZEN behaviour table of Gitea #2365, rows F1-F9 (`FieldKind`) and
//! C1-C11 (`OptionChoice`). Written from the issue table before the code.
//! Coders never edit this file; a wrong row is reported to the manager.
//! Place: crates/synapse-a11y/src/field_kind_table_2365.rs (registered by
//! the `#[path]` line at the top of field_kind.rs). One test per row, named
//! after the row. Extra control cases (marked CONTROL) stop a stub that
//! refuses everything, or gives one fixed answer, from passing a row.

use super::{FieldKind, OptionChoice, SetMethod};

fn input(input_type: Option<&str>) -> FieldKind {
    FieldKind::new("input", input_type, false, false)
}

/// The refusal text, or a failure naming what came instead.
fn refusal(kind: &FieldKind) -> Result<String, String> {
    match kind.set_method() {
        SetMethod::Refused(text) => Ok(text),
        other => Err(format!("{kind:?}: expected a refusal, got {other:?}")),
    }
}

fn options(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(label, value)| ((*label).to_owned(), (*value).to_owned()))
        .collect()
}

/// Food/1, Rent/2, Power/3.
fn bills() -> Vec<(String, String)> {
    options(&[("Food", "1"), ("Rent", "2"), ("Power", "3")])
}

fn pick(
    option: Option<&str>,
    option_index: Option<i32>,
    list: &[(String, String)],
) -> Result<usize, String> {
    OptionChoice::new(option, option_index)?.pick(list)
}

#[test]
fn f1_text_like_fields_are_typed() {
    for input_type in [
        Some("text"),
        Some("search"),
        Some("email"),
        Some("url"),
        Some("tel"),
        Some("password"),
        Some("number"),
        None,
    ] {
        assert_eq!(
            input(input_type).set_method(),
            SetMethod::Typed,
            "input type {input_type:?}"
        );
    }
    assert_eq!(
        FieldKind::new("textarea", None, false, false).set_method(),
        SetMethod::Typed,
        "textarea"
    );
}

#[test]
fn f2_date_like_fields_take_a_value() {
    for input_type in [
        "date",
        "time",
        "datetime-local",
        "month",
        "week",
        "color",
        "range",
    ] {
        assert_eq!(
            input(Some(input_type)).set_method(),
            SetMethod::Value,
            "input type {input_type}"
        );
    }
}

#[test]
fn f3_select_is_refused_pointing_to_act_select() -> Result<(), String> {
    let text = refusal(&FieldKind::new("select", None, false, false))?;
    assert!(text.contains("act select"), "{text}");
    Ok(())
}

#[test]
fn f4_checkbox_and_radio_are_refused_pointing_to_check_uncheck_click() -> Result<(), String> {
    for input_type in ["checkbox", "radio"] {
        let text = refusal(&input(Some(input_type)))?;
        for word in ["check", "uncheck", "click"] {
            assert!(text.contains(word), "{input_type}: {text}");
        }
        // CONTROL: not the dropdown advice.
        assert!(!text.contains("act select"), "{input_type}: {text}");
    }
    Ok(())
}

#[test]
fn f5_file_is_refused_pointing_to_the_upload_tool() -> Result<(), String> {
    let text = refusal(&input(Some("file")))?;
    assert!(text.contains("upload"), "{text}");
    Ok(())
}

#[test]
fn f6_hidden_disabled_and_read_only_are_refused_not_editable() -> Result<(), String> {
    let locked = [
        input(Some("hidden")),
        FieldKind::new("input", Some("text"), false, true),
        FieldKind::new("input", Some("date"), false, true),
        FieldKind::new("textarea", None, false, true),
        FieldKind::new("select", None, false, true),
        FieldKind::new("div", None, true, true),
    ];
    for kind in &locked {
        let text = refusal(kind)?;
        assert!(text.contains("not editable"), "{kind:?}: {text}");
    }
    Ok(())
}

#[test]
fn f7_content_editable_div_is_typed() {
    assert_eq!(
        FieldKind::new("div", None, true, false).set_method(),
        SetMethod::Typed
    );
}

#[test]
fn f8_capitals_and_unknown_types_read_as_lower_case_text() -> Result<(), String> {
    assert_eq!(
        FieldKind::new("INPUT", Some("Date"), false, false).set_method(),
        SetMethod::Value
    );
    assert_eq!(
        FieldKind::new("INPUT", Some("TEXT"), false, false).set_method(),
        SetMethod::Typed
    );
    assert_eq!(
        FieldKind::new("TEXTAREA", None, false, false).set_method(),
        SetMethod::Typed
    );
    let text = refusal(&FieldKind::new("SELECT", None, false, false))?;
    assert!(text.contains("act select"), "{text}");
    assert_eq!(
        input(Some("foo")).set_method(),
        SetMethod::Typed,
        "unknown type"
    );
    Ok(())
}

#[test]
fn f9_plain_div_is_refused_not_an_editable_field() -> Result<(), String> {
    let text = refusal(&FieldKind::new("div", None, false, false))?;
    assert!(text.contains("not an editable field"), "{text}");
    Ok(())
}

#[test]
fn c1_label_picks_its_index() {
    assert_eq!(pick(Some("Rent"), None, &bills()), Ok(1));
}

#[test]
fn c2_value_picks_its_index_when_no_label_matches() {
    assert_eq!(pick(Some("2"), None, &bills()), Ok(1));
}

#[test]
fn c3_label_is_trimmed_and_case_insensitive() {
    assert_eq!(pick(Some(" rent "), None, &bills()), Ok(1));
}

#[test]
fn c4_index_picks_itself() {
    assert_eq!(pick(None, Some(2), &bills()), Ok(2));
}

#[test]
fn c5_index_out_of_range_is_refused_naming_the_count() -> Result<(), String> {
    for index in [3, -1] {
        let Err(text) = pick(None, Some(index), &bills()) else {
            return Err(format!("option_index {index} was accepted"));
        };
        assert!(text.contains("out of range"), "{index}: {text}");
        assert!(text.contains("3 options"), "{index}: {text}");
    }
    Ok(())
}

#[test]
fn c6_missing_label_is_refused_listing_the_labels() -> Result<(), String> {
    let Err(text) = pick(Some("Water"), None, &bills()) else {
        return Err("Water was accepted".to_owned());
    };
    for label in ["Food", "Rent", "Power"] {
        assert!(text.contains(label), "{text}");
    }
    Ok(())
}

#[test]
fn c7_two_options_with_the_label_are_refused() -> Result<(), String> {
    let twins = options(&[("Rent", "2"), ("Rent", "9")]);
    let Err(text) = pick(Some("Rent"), None, &twins) else {
        return Err("an ambiguous label was accepted".to_owned());
    };
    assert!(text.contains("2 options have the label"), "{text}");
    // CONTROL: the same label once is picked.
    assert_eq!(
        pick(
            Some("Rent"),
            None,
            &options(&[("Rent", "2"), ("Food", "9")])
        ),
        Ok(0)
    );
    Ok(())
}

#[test]
fn c8_empty_list_is_refused() -> Result<(), String> {
    let Err(text) = pick(Some("Rent"), None, &[]) else {
        return Err("a pick from no options was accepted".to_owned());
    };
    assert!(text.contains("no options"), "{text}");
    Ok(())
}

#[test]
fn c9_long_list_shows_twenty_labels_and_counts_the_rest() -> Result<(), String> {
    let long: Vec<(String, String)> = (1..=25)
        .map(|n| (format!("Opt{n:02}"), format!("v{n:02}")))
        .collect();
    let Err(text) = pick(Some("Water"), None, &long) else {
        return Err("Water was accepted".to_owned());
    };
    assert!(text.contains("Opt01"), "{text}");
    assert!(text.contains("Opt20"), "{text}");
    assert!(!text.contains("Opt21"), "{text}");
    assert!(text.contains("5 more"), "{text}");
    Ok(())
}

#[test]
fn c10_label_wins_over_value() {
    let list = options(&[("2", "a"), ("Rent", "b"), ("Power", "2")]);
    assert_eq!(pick(Some("2"), None, &list), Ok(0));
}

#[test]
fn c11_no_option_and_no_index_is_refused() -> Result<(), String> {
    for blank in [None, Some(""), Some("   ")] {
        let Err(text) = OptionChoice::new(blank, None) else {
            return Err(format!("{blank:?} without an index was accepted"));
        };
        assert!(text.contains("option is needed"), "{blank:?}: {text}");
    }
    Ok(())
}
