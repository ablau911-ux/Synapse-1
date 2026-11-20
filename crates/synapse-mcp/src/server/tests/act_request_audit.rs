//! Regression tests for #51: act_* started audit rows carry the sanitized
//! request identity so CF_ACTION_LOG can reconstruct what an action did.

use super::*;

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => panic!("test params must deserialize: {error}"),
    }
}

#[test]
fn must_pass_click_request_records_point_geometry_and_button() {
    let params: ActClickParams = parse(json!({ "target": { "x": 640, "y": -12 }, "clicks": 2 }));
    let request = ActRequestAudit::click(&params);
    assert_eq!(
        request["target"],
        json!({ "kind": "point", "x": 640, "y": -12 })
    );
    assert_eq!(request["clicks"], json!(2));
    // No button given: the recorded label is the schema default, MouseButton::Left.
    assert_eq!(request["button"], json!("Left"));
}

#[test]
fn must_pass_press_request_records_named_key_labels() {
    let params: ActPressParams = parse(json!({ "keys": ["ctrl", "shift", "end"] }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(request["keys"], json!(["ctrl", "shift", "end"]));
    assert_eq!(request["key_count"], json!(3));
}

#[test]
fn must_fail_press_request_redacts_printable_keystrokes() {
    for (keys, plain) in [
        (json!(["h"]), "\"h\""),
        (json!(["shift", "H"]), "\"H\""),
        (json!(["\u{05e9}"]), "\u{05e9}"),
    ] {
        let params: ActPressParams = parse(json!({ "keys": keys }));
        let request = ActRequestAudit::press(&params);
        let rendered = request["keys"].to_string();
        assert!(rendered.contains("<redacted:"), "{rendered}");
        assert!(!rendered.contains(plain), "{rendered}");
    }
}

#[test]
fn must_fail_press_request_keeps_no_reversible_hash_of_a_single_keystroke() {
    let params: ActPressParams = parse(json!({ "keys": ["a"] }));
    let request = ActRequestAudit::press(&params);
    let rendered = request.to_string();
    for candidate in ["a", "A"] {
        assert!(!rendered.contains(&text_signature(candidate)), "{rendered}");
    }
    assert!(request.get("keys_sha256").is_none());
}

#[test]
fn must_fail_press_request_redacts_altgr_chords_that_type_text() {
    let params: ActPressParams = parse(json!({ "keys": ["ctrl", "alt", "q"] }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(
        request["keys"],
        json!(["ctrl", "alt", REDACTED_PRINTABLE_KEY])
    );
}

#[test]
fn must_fail_press_request_redacts_unrecognized_key_names() {
    let params: ActPressParams = parse(json!({ "keys": ["ctrl", "hunter2"] }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(request["keys"], json!(["ctrl", REDACTED_UNRECOGNIZED_KEY]));
    assert!(!request.to_string().contains("hunter2"));
}

#[test]
fn must_pass_press_request_canonicalizes_named_key_aliases() {
    let params: ActPressParams = parse(json!({ "keys": ["Control", "Return"] }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(request["keys"], json!(["ctrl", "enter"]));
}

#[test]
fn must_pass_press_request_keeps_shortcut_chords_auditable() {
    let params: ActPressParams = parse(json!({ "keys": ["ctrl", "c"] }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(request["keys"], json!(["ctrl", "c"]));
}

#[test]
fn must_fail_press_request_bounds_oversized_key_lists_and_labels() {
    let many: Vec<String> = (0..10_000).map(|index| format!("F{index}")).collect();
    let params: ActPressParams = parse(json!({ "keys": many }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(
        request["keys"].as_array().map(Vec::len),
        Some(MAX_AUDIT_KEYS)
    );
    assert_eq!(request["key_count"], json!(10_000));
    let params: ActPressParams = parse(json!({ "keys": ["X".repeat(900_000)] }));
    let request = ActRequestAudit::press(&params);
    assert!(request.to_string().len() < 1_024);
}

#[test]
fn must_pass_click_request_records_hold_and_duration() {
    let params: ActClickParams =
        parse(json!({ "target": { "x": 1, "y": 2 }, "hold_ms": 30_000, "duration_ms": 5 }));
    let request = ActRequestAudit::click(&params);
    assert_eq!(request["hold_ms"], json!(30_000));
    assert_eq!(request["duration_ms"], json!(5));
    // No profile given: the recorded label is the schema default, Natural.
    assert_eq!(request["velocity_profile"], json!("Natural"));
}

#[test]
fn must_pass_pad_request_records_inputs() {
    let params: ActPadParams = parse(json!({ "report": { "lt": 0.5 }, "hold_ms": 250 }));
    let request = ActRequestAudit::pad(&params);
    assert_eq!(
        request["pad_id"],
        json!(format!("{:?}", synapse_core::PadId::default()))
    );
    assert_eq!(request["controller"], json!("X360"));
    assert_eq!(request["backend"], json!("Vigem"));
    assert_eq!(request["hold_ms"], json!(250));
    let report = request["report"].as_str().expect("report label");
    assert!(report.contains("lt: 0.5"), "{report}");
    assert!(report.chars().count() <= MAX_AUDIT_DEBUG_CHARS + "...[truncated]".len());
}

#[test]
fn must_fail_type_request_records_length_and_hash_never_plaintext() {
    let secret = "hunter2-\u{05e1}\u{05d5}\u{05d3}";
    let params: ActTypeParams = parse(json!({ "text": secret }));
    let request = ActRequestAudit::type_text(&params);
    assert_eq!(request["requested_len"], json!(secret.chars().count()));
    assert_eq!(request["requested_sha256"], json!(text_signature(secret)));
    assert!(!request.to_string().contains("hunter2"));
}

#[test]
fn must_fail_type_request_handles_empty_text() {
    let params: ActTypeParams = parse(json!({ "text": "" }));
    let request = ActRequestAudit::type_text(&params);
    assert_eq!(request["requested_len"], json!(0));
    assert_eq!(request["requested_sha256"], json!(text_signature("")));
}

#[test]
fn must_pass_scroll_request_records_deltas_and_anchor() {
    let params: ActScrollParams = parse(json!({ "dy": -3, "at": { "x": 10, "y": 20 } }));
    let request = ActRequestAudit::scroll(&params);
    assert_eq!(request["dy"], json!(-3));
    assert_eq!(request["at"], json!({ "x": 10, "y": 20 }));
    assert_eq!(request["element_id"], Value::Null);
}

#[test]
fn must_pass_started_details_nest_request_next_to_preflight() {
    let details =
        ActRequestAudit::started_details(json!({ "keys": ["tab"] }), &json!({ "ok": true }));
    assert_eq!(details["request"]["keys"], json!(["tab"]));
    assert_eq!(details["preflight"]["ok"], json!(true));
}

#[test]
fn must_fail_press_request_redacts_space_outside_shortcuts() {
    let params: ActPressParams = parse(json!({ "keys": ["space"] }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(request["keys"], json!([REDACTED_PRINTABLE_KEY]));
    let params: ActPressParams = parse(json!({ "keys": ["ctrl", "space"] }));
    let request = ActRequestAudit::press(&params);
    assert_eq!(request["keys"], json!(["ctrl", "space"]));
}

#[test]
fn must_fail_rejected_key_names_are_not_echoed_in_errors() {
    let secret = "hunter2-secret";
    let error = crate::m2::press::canonical_key_name(secret).err();
    let rendered = format!("{error:?}");
    let code = error
        .as_ref()
        .and_then(|error| error.data.as_ref())
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str);
    assert_eq!(
        code,
        Some(synapse_core::error_codes::ACTION_UNSUPPORTED_KEY),
        "{rendered}"
    );
    assert!(!rendered.contains(secret), "{rendered}");
    assert!(rendered.contains(&format!("{} chars", secret.chars().count())));
}

#[test]
fn must_fail_press_request_redacts_text_keys_in_invalid_or_multi_key_chords() {
    for keys in [
        json!(["s", "e", "c", "r", "e", "t", "ctrl"]),
        json!(["ctrl", "p", "w"]),
        json!(["ctrl", "a", "a"]),
        json!(["ctrl", "a", "not-a-key"]),
    ] {
        let params: ActPressParams = parse(json!({ "keys": keys }));
        let rendered = ActRequestAudit::press(&params)["keys"].to_string();
        for letter in ["\"s\"", "\"e\"", "\"p\"", "\"w\"", "\"a\""] {
            assert!(!rendered.contains(letter), "{rendered}");
        }
    }
}
