//! #218: a no-delta error after delivered input says the input was delivered
//! and warns against a blind retry; an unproven delivery never claims it.

use serde_json::json;
use synapse_core::error_codes;

use super::{
    DELIVERED_NO_DELTA_RETRY_HINT, delivered_no_observed_delta_error, no_observed_delta_error,
};

#[test]
fn must_pass_a_delivered_click_without_effect_is_delivered_but_unverified() {
    let error = delivered_no_observed_delta_error(
        "act_click",
        "target_window_ui_or_pixels",
        250,
        "before".to_owned(),
        "after".to_owned(),
        json!({}),
    );
    let data = error.data.unwrap_or_default();
    assert_eq!(data["code"], error_codes::ACTION_NO_OBSERVED_DELTA);
    assert_eq!(data["delivered"], true);
    assert_eq!(data["retry_hint"], DELIVERED_NO_DELTA_RETRY_HINT);
}

#[test]
fn must_fail_an_unproven_delivery_never_claims_delivered() {
    let error = no_observed_delta_error(
        "act_type",
        "uia_value",
        250,
        "before".to_owned(),
        "after".to_owned(),
        json!({}),
    );
    let data = error.data.unwrap_or_default();
    assert_eq!(data["code"], error_codes::ACTION_NO_OBSERVED_DELTA);
    assert!(data.get("delivered").is_none());
    assert!(data.get("retry_hint").is_none());
}
