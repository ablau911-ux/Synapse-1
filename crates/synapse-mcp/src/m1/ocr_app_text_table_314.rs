//! FROZEN behaviour table of Gitea #314 (roadmap #618), rows A1-A7
//! (`AppTextCheck::from_nodes`), B1-B17 plus B7 line terminators (`AppTextCheck::new` + `apply`) and
//! C1-C4 (`ResolvedReadTextRequest::reported_lang`). Written from the table
//! before the code. Coders never edit this file; a wrong row is reported to
//! the manager. Place: crates/synapse-mcp/src/m1/ocr_app_text_table_314.rs
//! (registered by the `#[path]` lines at the top of `ocr_app_text.rs`). One
//! test per row, named after the row. A row whose main fact is "the reply is
//! unchanged" carries a CONTROL case that must change, so a do-nothing stub
//! cannot pass it. Boxes of B1 are the SY-03 baseline reply, of B2 the FM-02
//! observe reply, of B3 the FM-10 reply.

use synapse_core::{
    AccessibleNode, ConfidenceSource, ElementId, OcrBackend, OcrResult, OcrWord, Point, Rect,
};

use super::AppTextCheck;
use crate::m1::{ReadTextCaptureSource, ResolvedReadTextRequest};

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

/// An OCR word as Windows OCR gives it today (#311: 0.0, unsupported).
fn word(text: &str, bbox: Rect) -> OcrWord {
    OcrWord {
        text: text.to_owned(),
        bbox,
        confidence: 0.0,
        confidence_source: ConfidenceSource::Unsupported,
    }
}

fn reply(words: Vec<OcrWord>) -> OcrResult {
    OcrResult {
        full_text: words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        words,
        confidence: 0.0,
        confidence_source: ConfidenceSource::Unsupported,
        region: rect(0, 0, 400, 70),
        lang: "en-US".to_owned(),
        cache_write: None,
        perceived_text_notice: None,
        suspected_injection: Vec::new(),
    }
}

fn check(pairs: &[(&str, Rect)]) -> AppTextCheck {
    AppTextCheck::new(
        pairs
            .iter()
            .map(|(text, bbox)| ((*text).to_owned(), *bbox))
            .collect(),
    )
}

fn node(name: &str, value: Option<&str>, bbox: Rect) -> Result<AccessibleNode, String> {
    Ok(AccessibleNode {
        element_id: ElementId::parse("0x1:2a").map_err(|error| error.to_string())?,
        parent: None,
        name: name.to_owned(),
        role: "text".to_owned(),
        automation_id: None,
        value: value.map(str::to_owned),
        bbox,
        enabled: true,
        focused: false,
        patterns: Vec::new(),
        children_count: 0,
        depth: 1,
        control_state: None,
    })
}

fn texts(result: &OcrResult) -> Vec<&str> {
    result.words.iter().map(|word| word.text.as_str()).collect()
}

fn only_word(result: &OcrResult) -> Result<&OcrWord, String> {
    match result.words.as_slice() {
        [single] => Ok(single),
        other => Err(format!("expected one word, got {other:?}")),
    }
}

fn is_one(value: f32) -> bool {
    (value - 1.0).abs() < f32::EPSILON
}

/// SY-03: the `WinForms` label "Synth 7395 label" read as "synth 7395 label".
fn sy03() -> OcrResult {
    reply(vec![
        word("synth", rect(31, 38, 63, 28)),
        word("7395", rect(103, 37, 54, 21)),
        word("label", rect(167, 34, 52, 22)),
    ])
}

/// The label box of SY-03 in reply coordinates.
const SY03_LABEL: Rect = rect(25, 30, 200, 40);

fn request(lang_hint: Option<&str>) -> ResolvedReadTextRequest {
    ResolvedReadTextRequest {
        region: rect(0, 0, 400, 70),
        capture_source: ReadTextCaptureSource::Screen,
        requested_backend: OcrBackend::Auto,
        effective_backend: OcrBackend::Winrt,
        lang_hint: lang_hint.map(str::to_owned),
        synthetic: false,
    }
}

// ---------------------------------------------------------------- A: from_nodes

#[test]
fn a1_label_name_is_read_at_its_screen_place() -> Result<(), String> {
    // The label's screen box (1025, 530) is (25, 30) in a reply whose (0, 0)
    // is the screen point (1000, 500).
    let label = node("Synth 7395 label", None, rect(1025, 530, 200, 40))?;
    let result = AppTextCheck::from_nodes(std::slice::from_ref(&label), Point { x: 1000, y: 500 })
        .apply(sy03());
    assert_eq!(result.full_text, "Synth 7395 label");
    // CONTROL: the same node with the wrong origin is not at the words' place.
    let unshifted = AppTextCheck::from_nodes(&[label], Point { x: 0, y: 0 }).apply(sy03());
    assert_eq!(unshifted, sy03());
    Ok(())
}

#[test]
fn a2_a_non_empty_value_wins_over_the_name() -> Result<(), String> {
    let amount = node("Amount", Some("123.45"), rect(10, 10, 80, 20))?;
    let result = AppTextCheck::from_nodes(&[amount], Point { x: 0, y: 0 })
        .apply(reply(vec![word("123.4S", rect(12, 14, 50, 12))]));
    assert_eq!(texts(&result), ["123.45"]);
    Ok(())
}

#[test]
fn a3_an_empty_value_falls_back_to_the_name() -> Result<(), String> {
    let search = node("Search", Some("  "), rect(10, 10, 80, 20))?;
    let result = AppTextCheck::from_nodes(&[search], Point { x: 0, y: 0 })
        .apply(reply(vec![word("Searcn", rect(12, 14, 50, 12))]));
    assert_eq!(texts(&result), ["Search"]);
    Ok(())
}

#[test]
fn a4_a_node_with_no_text_changes_nothing() -> Result<(), String> {
    let ocr = reply(vec![word("Searcn", rect(12, 14, 50, 12))]);
    let blank = node("", None, rect(10, 10, 80, 20))?;
    let empty_value = node("   ", Some(""), rect(10, 10, 80, 20))?;
    let result = AppTextCheck::from_nodes(&[blank.clone(), empty_value], Point { x: 0, y: 0 })
        .apply(ocr.clone());
    assert_eq!(result, ocr);
    // CONTROL: a named node at the same place is used.
    let named = node("Search", None, rect(10, 10, 80, 20))?;
    let result = AppTextCheck::from_nodes(&[blank, named], Point { x: 0, y: 0 }).apply(ocr);
    assert_eq!(texts(&result), ["Search"]);
    Ok(())
}

#[test]
fn a5_node_text_is_cleaned_of_marks_and_extra_spaces() -> Result<(), String> {
    let marked = node("\u{2066}-\u{20AA}528.53\u{2069}", None, rect(0, 0, 154, 36))?;
    let result = AppTextCheck::from_nodes(&[marked], Point { x: 0, y: 0 })
        .apply(reply(vec![word("-ru528.5\u{E4}", rect(5, 0, 149, 32))]));
    assert_eq!(texts(&result), ["-\u{20AA}528.53"]);
    let spaced = node("  Synth \n 7395\tlabel ", None, SY03_LABEL)?;
    let result = AppTextCheck::from_nodes(&[spaced], Point { x: 0, y: 0 }).apply(sy03());
    assert_eq!(result.full_text, "Synth 7395 label");
    Ok(())
}

#[test]
fn a6_a_node_box_with_no_area_is_left_out() -> Result<(), String> {
    let flat = node("Synth 7395 label", None, rect(25, 30, 0, 40))?;
    let result = AppTextCheck::from_nodes(&[flat], Point { x: 0, y: 0 }).apply(sy03());
    assert_eq!(result, sy03());
    // CONTROL: the same node with a width is used.
    let label = node("Synth 7395 label", None, SY03_LABEL)?;
    let result = AppTextCheck::from_nodes(&[label], Point { x: 0, y: 0 }).apply(sy03());
    assert_eq!(result.full_text, "Synth 7395 label");
    Ok(())
}

#[test]
fn a7_nodes_from_a_whole_tree_pick_their_own_words() -> Result<(), String> {
    // A window node (tall), a pane (tall) and the label: only the label explains
    // the words; the window and pane names never appear.
    let nodes = [
        node("Form1", None, rect(0, 0, 400, 300))?,
        node("Panel", None, rect(10, 20, 300, 200))?,
        node("Synth 7395 label", None, SY03_LABEL)?,
    ];
    let result = AppTextCheck::from_nodes(&nodes, Point { x: 0, y: 0 }).apply(sy03());
    assert_eq!(texts(&result), ["Synth 7395 label"]);
    Ok(())
}

// ---------------------------------------------------------------- B: apply

#[test]
fn b1_sy03_the_label_text_replaces_the_misread_words() -> Result<(), String> {
    let result = check(&[("Synth 7395 label", SY03_LABEL)]).apply(sy03());
    assert_eq!(result.full_text, "Synth 7395 label");
    let merged = only_word(&result)?;
    assert_eq!(merged.text, "Synth 7395 label");
    // The box of the words it replaces (their union), not the label box.
    assert_eq!(merged.bbox, rect(31, 34, 188, 32));
    assert_eq!(merged.confidence_source, ConfidenceSource::Uia);
    assert!(is_one(merged.confidence), "{}", merged.confidence);
    assert_eq!(result.confidence_source, ConfidenceSource::Uia);
    assert!(is_one(result.confidence), "{}", result.confidence);
    // Everything else of the reply stays.
    assert_eq!(result.region, rect(0, 0, 400, 70));
    assert_eq!(result.lang, "en-US");
    Ok(())
}

#[test]
fn b2_hebrew_text_comes_from_the_app() -> Result<(), String> {
    let ocr = reply(vec![
        word("monn", rect(1384, 240, 56, 9)),
        word("'\"no", rect(1446, 238, 40, 11)),
    ]);
    let total = "\u{05E1}\u{05D4}\"\u{05DB} \u{05D4}\u{05DB}\u{05E0}\u{05E1}\u{05D5}\u{05EA}";
    let result = check(&[(total, rect(1384, 231, 103, 22))]).apply(ocr);
    assert_eq!(result.full_text, total);
    assert_eq!(only_word(&result)?.confidence_source, ConfidenceSource::Uia);
    Ok(())
}

#[test]
fn b3_fm10_the_shekel_amount_comes_from_the_page() -> Result<(), String> {
    let ocr = reply(vec![word("-ru528.5\u{E4}", rect(5, 0, 149, 32))]);
    let result = check(&[("\u{2066}-\u{20AA}528.53\u{2069}", rect(0, 0, 154, 36))]).apply(ocr);
    assert_eq!(result.full_text, "-\u{20AA}528.53");
    assert_eq!(only_word(&result)?.confidence_source, ConfidenceSource::Uia);
    Ok(())
}

#[test]
fn b4_a_mixed_reply_keeps_ocr_words_and_says_least_trusted() {
    let mut words = vec![word("Form1", rect(5, 2, 40, 12))];
    words.extend(sy03().words);
    let result = check(&[("Synth 7395 label", SY03_LABEL)]).apply(reply(words));
    assert_eq!(texts(&result), ["Form1", "Synth 7395 label"]);
    assert_eq!(result.full_text, "Form1 Synth 7395 label");
    assert_eq!(
        result.words.first().map(|word| word.confidence_source),
        Some(ConfidenceSource::Unsupported)
    );
    // Mean of the word confidences (0.0 and 1.0); the least trusted source.
    assert!(
        (result.confidence - 0.5).abs() < f32::EPSILON,
        "{}",
        result.confidence
    );
    assert_eq!(result.confidence_source, ConfidenceSource::Unsupported);
}

#[test]
fn b5_a_tall_box_window_or_pane_does_not_replace() {
    let ocr = reply(vec![word("88.20", rect(10, 40, 40, 12))]);
    let result = check(&[("Form1", rect(0, 0, 400, 70))]).apply(ocr.clone());
    assert_eq!(result, ocr);
    // CONTROL: a label box around the same word does.
    let result = check(&[
        ("Form1", rect(0, 0, 400, 70)),
        ("88.20", rect(8, 38, 46, 16)),
    ])
    .apply(ocr);
    assert_eq!(
        result.words.first().map(|word| word.confidence_source),
        Some(ConfidenceSource::Uia)
    );
}

#[test]
fn b6_a_much_longer_text_does_not_replace() {
    let ocr = reply(vec![
        word("synapse", rect(527, 117, 66, 13)),
        word("line", rect(604, 113, 37, 13)),
        word("Is", rect(652, 115, 12, 11)),
    ]);
    let editor = "Editor content; Press Alt+F1 for Accessibility Options.";
    let result = check(&[(editor, rect(460, 105, 1340, 30))]).apply(ocr.clone());
    assert_eq!(result, ocr);
    // CONTROL: the line's own text in the same box does.
    let result = check(&[("synapse line 1s", rect(460, 105, 1340, 30))]).apply(ocr);
    assert_eq!(result.full_text, "synapse line 1s");
}

#[test]
fn b7_a_trailing_line_terminator_is_removed() -> Result<(), String> {
    let ocr = reply(vec![
        word("synapse", rect(12, 12, 60, 12)),
        word("line", rect(80, 12, 30, 12)),
        word("Is", rect(118, 12, 12, 12)),
    ]);
    for name in ["synapse line 1s\n", "synapse line 1s\r\n"] {
        let line = node(name, None, rect(8, 8, 145, 21))?;
        let result = AppTextCheck::from_nodes(&[line], Point { x: 0, y: 0 }).apply(ocr.clone());
        assert_eq!(result.full_text, "synapse line 1s", "{name:?}");
        assert_eq!(only_word(&result)?.confidence_source, ConfidenceSource::Uia);
    }
    Ok(())
}

#[test]
fn b7_an_icon_letter_is_not_replaced_by_the_button_name() {
    let ocr = reply(vec![word("B", rect(12, 12, 8, 12))]);
    let result = check(&[("Bold", rect(8, 8, 20, 20))]).apply(ocr.clone());
    assert_eq!(result, ocr);
    // CONTROL: a misread of the same length is replaced.
    let result = check(&[("Bold", rect(8, 8, 40, 20))])
        .apply(reply(vec![word("Bo1d", rect(12, 12, 30, 12))]));
    assert_eq!(texts(&result), ["Bold"]);
}

#[test]
fn b8_the_smallest_box_around_a_word_wins() {
    let ocr = reply(vec![word("synth", rect(12, 8, 40, 12))]);
    let result = check(&[
        ("Other", rect(0, 0, 400, 30)),
        ("Synth", rect(10, 5, 60, 20)),
    ])
    .apply(ocr.clone());
    assert_eq!(texts(&result), ["Synth"]);
    // Same answer whatever the order of the texts.
    let result = check(&[
        ("Synth", rect(10, 5, 60, 20)),
        ("Other", rect(0, 0, 400, 30)),
    ])
    .apply(ocr);
    assert_eq!(texts(&result), ["Synth"]);
}

#[test]
fn b9_two_labels_on_one_line_each_replace_their_own_word() {
    let ocr = reply(vec![
        word("Narne:", rect(10, 10, 40, 12)),
        word("Rcnt", rect(100, 10, 30, 12)),
    ]);
    let result = check(&[("Name:", rect(5, 5, 50, 20)), ("Rent", rect(95, 5, 60, 20))]).apply(ocr);
    assert_eq!(texts(&result), ["Name:", "Rent"]);
    assert!(
        result
            .words
            .iter()
            .all(|word| word.confidence_source == ConfidenceSource::Uia)
    );
    assert_eq!(result.confidence_source, ConfidenceSource::Uia);
}

#[test]
fn b10_two_boxes_of_the_same_size_the_first_wins() {
    let ocr = reply(vec![word("synth", rect(12, 8, 40, 12))]);
    let result = check(&[
        ("Synth", rect(5, 5, 100, 20)),
        ("Other", rect(5, 5, 100, 20)),
    ])
    .apply(ocr);
    assert_eq!(texts(&result), ["Synth"]);
}

#[test]
fn b11_a_text_with_no_ocr_word_is_never_added() {
    let result = check(&[
        ("Hidden text", rect(300, 50, 80, 15)),
        ("Synth 7395 label", SY03_LABEL),
    ])
    .apply(sy03());
    assert_eq!(texts(&result), ["Synth 7395 label"]);
    assert!(!result.full_text.contains("Hidden"), "{}", result.full_text);
}

#[test]
fn b12_no_app_text_leaves_the_reply_exactly_as_it_was() {
    let mut ocr = sy03();
    ocr.cache_write = None;
    ocr.perceived_text_notice = Some("kept".to_owned());
    let result = check(&[]).apply(ocr.clone());
    assert_eq!(result, ocr);
    // CONTROL: with the label, only the text fields change; the rest stays.
    let result = check(&[("Synth 7395 label", SY03_LABEL)]).apply(ocr);
    assert_eq!(result.full_text, "Synth 7395 label");
    assert_eq!(result.perceived_text_notice.as_deref(), Some("kept"));
}

#[test]
fn b13_a_merged_word_takes_the_place_of_its_first_word() {
    // "Due" sits in its own small box inside the "Total 88.20" box.
    let ocr = reply(vec![
        word("Tota1", rect(10, 10, 40, 12)),
        word("Due", rect(60, 10, 30, 12)),
        word("88.2O", rect(100, 10, 40, 12)),
    ]);
    let result = check(&[
        ("Total 88.20", rect(5, 5, 150, 20)),
        ("Due", rect(58, 7, 34, 16)),
    ])
    .apply(ocr);
    assert_eq!(texts(&result), ["Total 88.20", "Due"]);
    assert_eq!(result.full_text, "Total 88.20 Due");
}

#[test]
fn b14_a_word_centre_on_the_right_or_bottom_edge_is_outside() {
    let box_ok = ("OK", rect(100, 100, 50, 20));
    // Centre (150, 110): on the right edge -> outside.
    let right = reply(vec![word("0K", rect(145, 104, 10, 12))]);
    assert_eq!(check(&[box_ok]).apply(right.clone()), right);
    // Centre (110, 120): on the bottom edge -> outside.
    let bottom = reply(vec![word("0K", rect(105, 114, 10, 12))]);
    assert_eq!(check(&[box_ok]).apply(bottom.clone()), bottom);
    // CONTROL: centre (100, 100) on the left and top edges -> inside.
    let left = reply(vec![word("0K", rect(95, 94, 10, 12))]);
    assert_eq!(texts(&check(&[box_ok]).apply(left)), ["OK"]);
}

#[test]
fn b15_length_factor_two_is_the_limit_both_ways() {
    let at = |ocr_text: &str, app_text: &str| {
        let ocr = reply(vec![word(ocr_text, rect(10, 10, 40, 12))]);
        let result = check(&[(app_text, rect(5, 5, 60, 20))]).apply(ocr);
        result.full_text
    };
    assert_eq!(at("ab", "abcd"), "abcd");
    assert_eq!(at("ab", "abcde"), "ab");
    assert_eq!(at("abcd", "ab"), "ab");
    assert_eq!(at("abcde", "ab"), "abcde");
    // Spaces and direction marks do not count: 4 letters against 2.
    assert_eq!(at("ab", "\u{2066}a b c d\u{2069}"), "a b c d");
}

#[test]
fn b16_height_factor_three_is_the_limit() {
    let ocr = reply(vec![word("Rent", rect(10, 10, 30, 12))]);
    // Tallest word 12: a box 36 high explains it, 37 does not.
    let result = check(&[("Rent", rect(5, 0, 60, 36))]).apply(ocr.clone());
    assert_eq!(
        result.words.first().map(|word| word.confidence_source),
        Some(ConfidenceSource::Uia)
    );
    let result = check(&[("Rent", rect(5, 0, 60, 37))]).apply(ocr.clone());
    assert_eq!(result, ocr);
}

#[test]
fn b17_hostile_texts_are_refused_or_cleaned_without_panic() {
    let ocr = reply(vec![word("xx", rect(10, 10, 20, 12))]);
    let huge = "x".repeat(10_000);
    assert_eq!(
        check(&[(huge.as_str(), rect(5, 5, 60, 20))]).apply(ocr.clone()),
        ocr
    );
    let marks_only = "\u{2066}\u{2069} \t ";
    assert_eq!(
        check(&[(marks_only, rect(5, 5, 60, 20))]).apply(ocr.clone()),
        ocr
    );
    assert_eq!(
        check(&[("xx", rect(5, 5, -60, 20))]).apply(ocr.clone()),
        ocr
    );
    // CONTROL: line breaks inside an app text become single spaces.
    let result = check(&[("Synth\r\n7395\tlabel", SY03_LABEL)]).apply(sy03());
    assert_eq!(result.full_text, "Synth 7395 label");
}

// ---------------------------------------------------------------- C: reported_lang

#[test]
fn c1_the_engine_language_is_named() {
    assert_eq!(request(None).reported_lang(Some("en-US")), "en-US");
}

#[test]
fn c2_the_engine_language_wins_over_another_hint() {
    assert_eq!(request(Some("en-GB")).reported_lang(Some("en-US")), "en-US");
}

#[test]
fn c3_no_engine_language_falls_back_to_the_hint_then_und() {
    assert_eq!(request(Some(" fr-FR ")).reported_lang(None), "fr-FR");
    assert_eq!(request(None).reported_lang(None), "und");
    assert_eq!(request(Some("   ")).reported_lang(None), "und");
}

#[test]
fn c4_a_blank_engine_language_counts_as_none() {
    assert_eq!(request(Some("fr-FR")).reported_lang(Some("  ")), "fr-FR");
    assert_eq!(request(None).reported_lang(Some("")), "und");
}
