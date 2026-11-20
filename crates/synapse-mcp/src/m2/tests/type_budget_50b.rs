//! #50 follow-up: the foreground typing time budget applies only to SendInput
//! paths; background/bridge typing keeps just the absolute size cap, and the
//! budget projects the real per-key cost of the dynamics tier.

use synapse_core::{KeystrokeDynamics, KeystrokeNaturalParams};

use super::{
    ActTypeParams, MAX_ACT_TYPE_UTF16_UNITS, action_from_foreground_type_params,
    action_from_type_params, keystroke_budget_iki_ms, validate_foreground_type_text_budget,
};

fn params(len: usize, dynamics: &str) -> ActTypeParams {
    serde_json::from_value(serde_json::json!({
        "text": "x".repeat(len),
        "dynamics": dynamics,
    }))
    .expect("synthetic act_type params must deserialize")
}

fn budget_reason(result: Result<(), rmcp::ErrorData>) -> Option<String> {
    result
        .err()
        .and_then(|error| error.data)
        .and_then(|data| data["reason"].as_str().map(str::to_owned))
}

fn fast() -> KeystrokeDynamics {
    KeystrokeDynamics::Natural {
        params: KeystrokeNaturalParams::FAST,
    }
}

#[test]
fn must_pass_background_1000_char_natural_type_is_not_refused_by_time_budget() {
    // Chrome-bridge typing and target_act type pre-routing use this entry.
    assert!(action_from_type_params(&params(1000, "natural")).is_ok());
    assert!(action_from_type_params(&params(MAX_ACT_TYPE_UTF16_UNITS, "natural")).is_ok());
}

#[test]
fn must_fail_background_entry_still_enforces_absolute_size_cap() {
    let error = action_from_type_params(&params(MAX_ACT_TYPE_UTF16_UNITS + 1, "natural"))
        .expect_err("over the absolute cap must be refused");
    let data = error.data.expect("size error is structured");
    assert_eq!(data["reason"], "type_text_request_too_large");
}

#[test]
fn must_fail_foreground_entry_still_refuses_over_budget_and_accepts_boundary() {
    let error = action_from_foreground_type_params(&params(1000, "natural"))
        .expect_err("1000 natural chars exceed the foreground budget");
    let data = error.data.expect("budget error is structured");
    assert_eq!(data["reason"], "foreground_type_budget_exceeded");
    assert_eq!(data["refused_before_delivery"], true);

    assert!(action_from_foreground_type_params(&params(589, "natural")).is_ok());
    let boundary = action_from_foreground_type_params(&params(590, "natural"))
        .expect_err("590 natural chars are one past the foreground budget");
    let data = boundary.data.expect("budget error is structured");
    assert_eq!(data["reason"], "foreground_type_budget_exceeded");
    assert_eq!(data["refused_before_delivery"], true);
}

#[test]
fn must_pass_per_key_cost_comes_from_the_dynamics_tier() {
    assert_eq!(keystroke_budget_iki_ms(&KeystrokeDynamics::Burst), 0);
    assert_eq!(
        keystroke_budget_iki_ms(&KeystrokeDynamics::Linear { ms_per_char: 40 }),
        40
    );
    assert_eq!(keystroke_budget_iki_ms(&fast()), 32);
}

#[test]
fn must_fail_fast_tier_budget_boundary_matches_32ms_not_62ms() {
    // 2 ms dispatch + 32 ms IKI: 589 units project 19_994 ms, 590 project 20_028 ms.
    let text = |n: usize| "x".repeat(n);
    assert!(validate_foreground_type_text_budget(&text(589), false, &fast()).is_ok());
    assert_eq!(
        budget_reason(validate_foreground_type_text_budget(
            &text(590),
            false,
            &fast()
        ))
        .as_deref(),
        Some("foreground_type_budget_exceeded")
    );
    // The old 62 ms assumption refused from 314 units; a 62 ms linear tier still does.
    let slow = KeystrokeDynamics::Linear { ms_per_char: 62 };
    assert!(validate_foreground_type_text_budget(&text(313), false, &slow).is_ok());
    assert_eq!(
        budget_reason(validate_foreground_type_text_budget(
            &text(314),
            false,
            &slow
        ))
        .as_deref(),
        Some("foreground_type_budget_exceeded")
    );
    assert!(validate_foreground_type_text_budget(&text(314), false, &fast()).is_ok());
}
