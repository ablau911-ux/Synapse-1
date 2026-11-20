use super::*;

fn date_signature(segment: &str, path: &str) -> ActTypeTextSignature {
    ActTypeTextSignature {
        foreground_hwnd: 42,
        foreground_pid: 77,
        foreground_process: "visual-test.exe".to_owned(),
        focused_element_id: Some(segment.to_owned()),
        focused_role: Some("spin button".to_owned()),
        readback_source: Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE.to_owned()),
        cdp_selected_target_id: Some("page-1".to_owned()),
        cdp_active_tag_name: Some("INPUT".to_owned()),
        cdp_active_input_type: Some("date".to_owned()),
        cdp_active_element_path: Some(path.to_owned()),
        ..Default::default()
    }
}

#[test]
fn must_pass_date_segments_share_the_underlying_dom_input() {
    let mut month = date_signature("month-uia", "0/1/3/2");
    month.focused_role = Some("region".to_owned());
    let year = date_signature("year-uia", "0/1/3/2");
    assert!(!act_type_text_target_changed(&month, &year));

    let other_input = date_signature("other-month-uia", "0/1/3/3");
    assert!(act_type_text_target_changed(&month, &other_input));

    let mut other_tab = year.clone();
    other_tab.cdp_selected_target_id = Some("page-2".to_owned());
    assert!(act_type_text_target_changed(&month, &other_tab));
}

#[test]
fn must_fail_date_value_must_match_the_complete_typed_date() {
    let date = date_signature("year-uia", "0/1/3/2");
    assert!(segmented_date_value_matches(
        &date,
        "2026-09-12",
        "09122026"
    ));
    assert!(!segmented_date_value_matches(
        &date,
        "2026-09-13",
        "09122026"
    ));
    assert!(!segmented_date_value_matches(
        &date,
        "2026-09-12",
        "0912202"
    ));

    let mut ordinary_input = date;
    ordinary_input.cdp_active_input_type = Some("text".to_owned());
    assert!(!segmented_date_value_matches(
        &ordinary_input,
        "2026-09-12",
        "09122026"
    ));
}

#[test]
fn must_pass_date_spin_segment_uses_complete_dom_value_instead_of_uia_segment() {
    let focused = ActTypeFocusedTextCandidate {
        element_id: "year-uia".to_owned(),
        role: "spin button".to_owned(),
        name: "Year".to_owned(),
        selected_text: None,
        bbox: Rect {
            x: 10,
            y: 10,
            w: 40,
            h: 24,
        },
        value: Some("2026".to_owned()),
        patterns: vec![UiaPattern::Value],
    };
    let cdp = CdpActiveTextReadback {
        value: Some("2026-09-12".to_owned()),
        target_id: Some("page-1".to_owned()),
        has_active_element: Some(true),
        is_editable: Some(true),
        tag_name: Some("INPUT".to_owned()),
        input_type: Some("date".to_owned()),
        element_path: Some("0/1/3/2".to_owned()),
        id_sha256: None,
        name_sha256: None,
        value_len: Some(10),
        value_sha256: None,
        error_code: None,
        error_detail_sha256: None,
        attempt: "available".to_owned(),
    };
    let ocr = OcrTextReadback {
        value: None,
        word_count: 0,
        value_len: None,
        value_sha256: None,
        attempt: "unavailable".to_owned(),
    };
    let (value, source) = choose_act_type_text_readback(
        Some(&focused),
        Some("2026".to_owned()),
        Some(ACT_TYPE_TEXT_SOURCE_UIA_VALUE),
        &cdp,
        &ocr,
    );
    assert_eq!(value.as_deref(), Some("2026-09-12"));
    assert_eq!(source.as_deref(), Some(ACT_TYPE_TEXT_SOURCE_CDP_ACTIVE));

    let mut other_input = cdp;
    other_input.input_type = Some("text".to_owned());
    let (value, source) = choose_act_type_text_readback(
        Some(&focused),
        Some("2026".to_owned()),
        Some(ACT_TYPE_TEXT_SOURCE_UIA_VALUE),
        &other_input,
        &ocr,
    );
    assert_eq!(value.as_deref(), Some("2026"));
    assert_eq!(source.as_deref(), Some(ACT_TYPE_TEXT_SOURCE_UIA_VALUE));
}
