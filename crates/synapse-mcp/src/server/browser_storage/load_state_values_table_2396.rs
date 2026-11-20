//! FROZEN behaviour table of Gitea #2396 (roadmap item #629), rows L1-L7.
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/server/browser_storage/load_state_values_table_2396.rs
//! (registered by the `#[cfg(test)] #[path]` line at the end of
//! load_state_validation.rs).
//!
//! The mask is read from the masker's own constants in browser_storage.rs
//! (`REDACTED_VALUE`, `VALUE_REDACTED_FIELD`), never copied. A refusal must
//! carry the code the neighbouring load_state validation errors carry
//! (`TOOL_PARAMS_INVALID`, `reject_before_mutation`) and the state path of the
//! masked entry. Rows L4, L5 and L7 expect "accepted", which a stub that
//! refuses nothing also gives, so each has a control case that must be
//! refused.

use serde_json::{Value, json};
use synapse_core::error_codes;

use super::super::{REDACTED_VALUE, VALUE_REDACTED_FIELD};
use super::{LoadStateValues, validate_payload};

/// The table's words: the refusal says a masked save cannot be restored.
const RESTORE_TEXT: &str = "a masked save cannot be restored";

struct Case {
    name: &'static str,
    state: Value,
    /// `None` = accepted; `Some((state path, name the text must quote))` =
    /// refused at that entry.
    refused: Option<(&'static str, &'static str)>,
}

fn cookie(name: &str, value: &str) -> Value {
    json!({"name": name, "value": value, "domain": "example.test", "path": "/"})
}

fn item(name: &str, value: &str) -> Value {
    json!({"name": name, "value": value})
}

/// `None` when the object accepts the state; else (state path, message).
fn refusal(state: &Value) -> Result<Option<(String, String)>, String> {
    let Some(error) = LoadStateValues::from_state(state).refusal() else {
        return Ok(None);
    };
    let data = error.data.as_ref().ok_or("refusal carries no data")?;
    if data["code"] != error_codes::TOOL_PARAMS_INVALID
        || data["partial_apply_policy"] != "reject_before_mutation"
        || data["operation"] != "load_state"
    {
        return Err(format!("refusal is not a load_state params error: {data}"));
    }
    let path = data["state_path"]
        .as_str()
        .ok_or_else(|| format!("refusal without state_path: {data}"))?;
    Ok(Some((path.to_owned(), error.message.to_string())))
}

fn check(case: &Case) -> Result<(), String> {
    let got = refusal(&case.state)?;
    match (case.refused, got) {
        (None, None) => Ok(()),
        (None, Some((path, message))) => Err(format!("refused at {path}: {message}")),
        (Some((path, _)), None) => Err(format!("accepted; expected refused at {path}")),
        (Some((path, name)), Some((got_path, message))) => {
            let quoted = format!("{name:?}");
            if got_path != path || !message.contains(&quoted) || !message.contains(RESTORE_TEXT) {
                return Err(format!(
                    "expected refusal at {path} naming {quoted} with {RESTORE_TEXT:?}; got {got_path}: {message}"
                ));
            }
            Ok(())
        }
    }
}

#[test]
fn load_state_values_table_2396() -> Result<(), String> {
    let mut masked_flag = cookie("sid", "abc123");
    masked_flag[VALUE_REDACTED_FIELD] = Value::Bool(true);
    let wrapped = format!("x{REDACTED_VALUE}y");
    let cases = [
        Case {
            name: "L1_cookie_with_the_mask_value",
            state: json!({"cookies": [cookie("sid", REDACTED_VALUE)]}),
            refused: Some(("state.cookies[0]", "sid")),
        },
        Case {
            name: "L2_cookie_with_a_normal_value_and_the_mask_flag",
            state: json!({"cookies": [masked_flag]}),
            refused: Some(("state.cookies[0]", "sid")),
        },
        Case {
            name: "L3_local_storage_item_with_the_mask_value",
            state: json!({"origins": [{"origin": "https://example.test",
                "localStorage": [item("token", REDACTED_VALUE)]}]}),
            refused: Some(("state.origins[0].localStorage[0]", "token")),
        },
        Case {
            name: "L3_session_storage_item_with_the_mask_value",
            state: json!({"origins": [{"origin": "https://example.test",
                "localStorage": [item("theme", "dark")],
                "sessionStorage": [item("draft", REDACTED_VALUE)]}]}),
            refused: Some(("state.origins[0].sessionStorage[0]", "draft")),
        },
        Case {
            name: "L4_normal_cookies_and_storage_items",
            state: json!({"cookies": [cookie("a", "1"), cookie("b", "2")],
                "origins": [{"origin": "https://example.test",
                    "localStorage": [item("k", "v")], "sessionStorage": [item("s", "t")]}]}),
            refused: None,
        },
        Case {
            name: "L4_control_same_state_one_storage_value_masked",
            state: json!({"cookies": [cookie("a", "1"), cookie("b", "2")],
                "origins": [{"origin": "https://example.test",
                    "localStorage": [item("k", REDACTED_VALUE)], "sessionStorage": [item("s", "t")]}]}),
            refused: Some(("state.origins[0].localStorage[0]", "k")),
        },
        Case {
            name: "L5_the_word_redacted_without_brackets",
            state: json!({"cookies": [cookie("plain", "redacted")]}),
            refused: None,
        },
        Case {
            name: "L5_the_mask_inside_a_longer_text",
            state: json!({"cookies": [cookie("long", &wrapped)],
                "origins": [{"origin": "https://example.test", "localStorage": [item("k", &wrapped)]}]}),
            refused: None,
        },
        Case {
            name: "L5_control_the_exact_mask_in_the_same_place",
            state: json!({"cookies": [cookie("plain", "redacted"), cookie("long", REDACTED_VALUE)]}),
            refused: Some(("state.cookies[1]", "long")),
        },
        Case {
            name: "L6_three_good_cookies_and_one_masked",
            state: json!({"cookies": [cookie("a", "1"), cookie("b", "2"), cookie("c", "3"),
                cookie("d", REDACTED_VALUE)]}),
            refused: Some(("state.cookies[3]", "d")),
        },
        Case {
            name: "L7_empty_state",
            state: json!({}),
            refused: None,
        },
        Case {
            name: "L7_state_without_a_cookies_key",
            state: json!({"origins": []}),
            refused: None,
        },
        Case {
            name: "L7_control_no_cookies_key_masked_session_item",
            state: json!({"origins": [{"origin": "https://example.test",
                "sessionStorage": [item("draft", REDACTED_VALUE)]}]}),
            refused: Some(("state.origins[0].sessionStorage[0]", "draft")),
        },
    ];
    let mut failures = Vec::new();
    for case in &cases {
        if let Err(detail) = check(case) {
            failures.push(format!("{}: {detail}", case.name));
        }
    }
    // L7 "as today": the existing validation still decides these states.
    if let Err(error) = validate_payload(&json!({})) {
        failures.push(format!(
            "L7_empty_state_validation: refused: {}",
            error.message
        ));
    }
    if let Err(error) = validate_payload(&json!({"origins": []})) {
        failures.push(format!(
            "L7_no_cookies_key_validation: refused: {}",
            error.message
        ));
    }
    match validate_payload(&json!([])) {
        Err(error) if error.message.contains("state must be an object") => {}
        other => failures.push(format!(
            "L7_not_an_object_validation: expected the existing refusal, got {:?}",
            other.map_err(|error| error.message)
        )),
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
