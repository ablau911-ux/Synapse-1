//! Frozen behaviour table for issue #301 row 301d: an absent storage value is reported as
//! absent, never fingerprinted as if it were the 4-byte text "null".
//!
//! Written by the manager from the table in issue #301 (work method v3), before the code.
//! Code agents must not edit this file.

use serde_json::{Map, Value, json};

use super::redact_browser_secret_fields;

fn fields(value: Value) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("value".to_owned(), value);
    map
}

#[test]
fn absent_value_table() {
    // absent (null): no fingerprint, no length, nothing counted as redacted
    let mut absent = fields(Value::Null);
    let count = redact_browser_secret_fields(&mut absent);
    assert_eq!(count, 0, "row absent_not_counted");
    assert!(
        !absent.contains_key("value_sha256"),
        "row absent_no_fingerprint"
    );
    assert!(!absent.contains_key("value_len"), "row absent_no_len");
    assert_eq!(
        absent.get("value"),
        Some(&Value::Null),
        "row absent_stays_null"
    );

    // present string: redacted with a keyed fingerprint and the real length (today's behaviour)
    let mut present = fields(json!("v-4821"));
    let count = redact_browser_secret_fields(&mut present);
    assert_eq!(count, 1, "row present_counted");
    assert_eq!(
        present.get("value"),
        Some(&json!("[redacted]")),
        "row present_redacted"
    );
    assert_eq!(present.get("value_len"), Some(&json!(6)), "row present_len");
    assert!(
        present
            .get("value_sha256")
            .and_then(Value::as_str)
            .is_some_and(|fp| fp.starts_with("hmac-sha256:")),
        "row present_fingerprint"
    );

    // the empty string is a real stored value: fingerprinted, length 0
    let mut empty = fields(json!(""));
    assert_eq!(
        redact_browser_secret_fields(&mut empty),
        1,
        "row empty_string_counted"
    );
    assert_eq!(
        empty.get("value_len"),
        Some(&json!(0)),
        "row empty_string_len"
    );

    // the text "null" is a real value and differs from absent
    let mut text_null = fields(json!("null"));
    assert_eq!(
        redact_browser_secret_fields(&mut text_null),
        1,
        "row text_null_is_value"
    );
}
