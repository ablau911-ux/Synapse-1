//! #220 binding decision: successful delegates need a final postcondition.
use super::*;
use serde_json::json;

#[test]
fn must_pass_readback_without_postcondition_is_delivered_unverified() {
    for result in [
        json!({"ok":true,"bounds":{"x":1,"y":2,"w":300,"h":200}}),
        json!({"ok":true,"exit_code":0,"stdout":"220"}),
    ] {
        let (_, ok, status, value) =
            target_act_delegate_response("variant", Ok(Json(result))).expect("successful delegate");
        assert_eq!(
            DeliveryClaim::new("set_window_bounds", ok, status, &value).state(),
            "delivered_unverified"
        );
    }
}

#[test]
fn must_pass_final_verified_state_keeps_verified_delivery() {
    let value = json!({"postcondition":{"status":"verified_state"}});
    assert_eq!(
        DeliveryClaim::new("set_window_bounds", true, "ok", &value).state(),
        "delivered_and_verified"
    );
}

#[test]
fn must_fail_nested_or_false_delta_cannot_supply_final_verification() {
    for value in [
        json!({"attempts":[{"postcondition":{"status":"verified_state"}}]}),
        json!({"postcondition":{"status":"observed_delta","observed_delta":false}}),
        json!({"postcondition":{"status":"not_requested"}}),
    ] {
        let before = value.clone();
        assert_eq!(
            DeliveryClaim::new("set_window_bounds", true, "ok", &value).state(),
            "delivered_unverified"
        );
        assert_eq!(value, before);
    }
}

#[test]
fn must_fail_refused_delegate_stays_before_delivery_even_with_verified_payload() {
    let value = json!({"postcondition":{"status":"verified_state"}});
    assert_eq!(
        DeliveryClaim::new("set_window_bounds", false, "refused", &value).state(),
        "refused_before_delivery"
    );
}
