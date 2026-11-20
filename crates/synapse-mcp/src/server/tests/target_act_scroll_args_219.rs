//! #219: the documented `target_act verb=scroll` example must scroll exactly ONE
//! wheel step. `args` are notches (the unit `act_scroll` multiplies by
//! WHEEL_DELTA=120 downstream), so the example must be +/-1, never 120.
//! Direction: positive dy scrolls UP on every route, so "scroll down" is dy < 0.

use super::{TARGET_ACT_SCROLL_DOWN_ONE_STEP_ARGS, parse_target_act_scroll_args};
use synapse_core::error_codes;

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

fn to_args(raw: [&str; 2]) -> Vec<String> {
    raw.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn must_pass_documented_example_is_one_notch_down() {
    let (dx, dy) = parse_target_act_scroll_args(&to_args(TARGET_ACT_SCROLL_DOWN_ONE_STEP_ARGS))
        .expect("documented example must parse");
    assert_eq!(dx, 0);
    assert_eq!(
        dy, -1,
        "one wheel step down is dy=-1 notch, not a raw 120 delta"
    );
}

#[test]
fn must_fail_empty_args_refusal_advertises_the_example() {
    let error = parse_target_act_scroll_args(&[]).expect_err("empty args must be rejected");
    assert_eq!(error_code(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    let [example_dx, example_dy] = TARGET_ACT_SCROLL_DOWN_ONE_STEP_ARGS;
    let expected = format!("args=[\"{example_dx}\",\"{example_dy}\"]");
    assert!(
        error.message.contains(&expected),
        "message {:?} must contain {expected}",
        error.message
    );
}

#[test]
fn must_fail_non_integer_args_are_rejected() {
    let error = parse_target_act_scroll_args(&to_args(["0", "abc"]))
        .expect_err("a non-integer delta must be rejected");
    assert_eq!(error_code(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    assert!(
        error.message.contains("\"abc\""),
        "the message names the offending value: {:?}",
        error.message
    );
}

#[test]
fn must_pass_explicit_notch_values_pass_through_unchanged() {
    assert_eq!(
        parse_target_act_scroll_args(&to_args(["2", "-3"])).expect("valid"),
        (2, -3)
    );
}
