//! #281 frozen intent: exact emitted text after an initially empty edit field.
//! Exercise the production act-type postcondition owner, not a test predicate.
use rmcp::ErrorData;

use super::super::{ActTypeResponse, ActTypeTextReadback, verify_act_type_text_response};
use super::{response, segment_signature};

fn text_readback(value: &str) -> ActTypeTextReadback {
    let mut signature = segment_signature(281);
    signature.focused_role = Some("edit".to_owned());
    ActTypeTextReadback {
        signature,
        value: Some(value.to_owned()),
        browser_url: None,
        date_parent: None,
    }
}

fn verified(after: &str) -> Result<ActTypeResponse, ErrorData> {
    let mut dispatched = response();
    dispatched.chars_typed = 3;
    verify_act_type_text_response(
        dispatched,
        text_readback(""),
        text_readback(after),
        "before-empty".to_owned(),
        format!("after:{after}"),
        250,
        "abc",
    )
}

#[test]
fn must_pass_exact_emitted_text_from_empty_field_verifies_281() {
    let result = verified("abc");
    assert!(
        result.is_ok(),
        "exact independent readback must verify: {result:?}"
    );
    if let Ok(result) = result {
        assert_eq!(result.postcondition.status, "observed_delta");
        assert!(!result.target_readback_required);
    }
}

#[test]
fn must_fail_missing_first_character_cannot_verify_dispatch_success_281() {
    assert!(
        verified("bc").is_err(),
        "dispatch success cannot mask a consumed first key"
    );
}

#[test]
fn must_fail_duplicate_prefix_cannot_verify_as_exact_emitted_text_281() {
    assert!(
        verified("aabc").is_err(),
        "prefix repair must not duplicate the first character"
    );
}

#[test]
fn must_fail_unexpected_suffix_cannot_verify_as_exact_emitted_text_281() {
    assert!(
        verified("abcc").is_err(),
        "substring containment is not exact delivered text"
    );
}

// r2 D3/D4: exercise the same production verifier with an existing value and
// an independently captured selection digest, including Win32 CRLF readback.
fn verify_existing(
    previous: &str,
    selected: Option<&str>,
    emitted: &str,
    actual: &str,
) -> Result<ActTypeResponse, ErrorData> {
    let mut before = text_readback(previous);
    before.signature.focused_selected_text_sha256 =
        selected.and_then(super::super::non_empty_sha256);
    let mut dispatched = response();
    dispatched.chars_typed = u32::try_from(emitted.chars().count()).unwrap_or(u32::MAX);
    verify_act_type_text_response(
        dispatched,
        before,
        text_readback(actual),
        "before-existing".to_owned(),
        "after-existing".to_owned(),
        250,
        emitted,
    )
}

#[test]
fn must_fail_existing_prefix_cannot_hide_dropped_first_key_281_r2_d3() {
    let result = verify_existing("x", None, "xy", "xy");
    assert!(
        result.is_err(),
        "x + xy with first x consumed leaves xy, not xxy; must reject: {result:?}"
    );
}

#[test]
fn must_pass_existing_prefix_plus_exact_insertion_281_r2_d3() {
    let result = verify_existing("x", None, "xy", "xxy");
    assert!(result.is_ok(), "full insertion must verify: {result:?}");
}

#[test]
fn must_pass_selected_entire_existing_value_can_be_replaced_281_r2_d3() {
    let result = verify_existing("x", Some("x"), "xy", "xy");
    assert!(
        result.is_ok(),
        "selection replacement must verify: {result:?}"
    );
}

#[test]
fn must_fail_unselected_entire_value_cannot_be_silently_replaced_281_r2_d3() {
    let result = verify_existing("retained", None, "xy", "xy");
    assert!(
        result.is_err(),
        "unselected content disappeared: {result:?}"
    );
}

#[test]
fn must_fail_crlf_selection_digest_must_not_reject_valid_replacement_281_r2_d4() {
    for (previous, selected, actual) in [
        (
            "head\r\nold\r\ntext\r\ntail",
            "old\r\ntext",
            "head\r\nnew\r\ntext\r\ntail",
        ),
        (
            "head\nold\ntext\ntail",
            "old\r\ntext",
            "head\nnew\ntext\ntail",
        ),
    ] {
        let result = verify_existing(previous, Some(selected), "new\ntext", actual);
        assert!(
            result.is_ok(),
            "same selected text with CRLF/LF must verify replacement: {result:?}"
        );
    }
}

#[test]
fn must_pass_lf_selection_replacement_keeps_surrounding_content_281_r2_d4() {
    let result = verify_existing(
        "head\nold\ntext\ntail",
        Some("old\ntext"),
        "new\ntext",
        "head\nnew\ntext\ntail",
    );
    assert!(
        result.is_ok(),
        "LF selection control must verify: {result:?}"
    );
}

#[test]
fn must_fail_wrong_multiline_selection_cannot_authorize_replacement_281_r2_d4() {
    let result = verify_existing(
        "head\r\nold\r\ntext\r\ntail",
        Some("other\r\nselection"),
        "new\ntext",
        "head\r\nnew\r\ntext\r\ntail",
    );
    assert!(
        result.is_err(),
        "unrelated selection cannot justify deletion: {result:?}"
    );
}
