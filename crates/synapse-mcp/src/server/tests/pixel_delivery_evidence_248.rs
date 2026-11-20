//! Child of background_router.rs: real final delivery-state classifier.
use super::*;

fn pixels() -> Value {
    json!({"status":"observed_delta", "source_of_truth":"target_point_pixels",
        "observed_delta":true, "before_signature":"a".repeat(64),
        "after_signature":"b".repeat(64)})
}

#[test]
fn must_pass_genuine_final_pixel_delta_is_pixel_verified() {
    let result = json!({"postcondition":pixels()});
    assert_eq!(result["postcondition"]["observed_delta"], json!(true));
    assert_eq!(
        DeliveryClaim::new("click", true, "ok", &result).state(),
        "delivered_and_pixel_verified"
    );
}

#[test]
fn must_fail_labels_nested_attempts_and_invalid_signatures_cannot_claim_pixels() {
    let mut cases = vec![
        json!({"source_of_truth":"target_window_ui_or_pixels"}),
        json!({"tier_attempts":[{"postcondition":pixels()}]}),
        json!({"postcondition":{"source_of_truth":"target_window_ui_or_pixels"}}),
    ];
    for (field, bad) in [
        ("status", json!("not_requested")),
        ("observed_delta", json!(false)),
        ("before_signature", Value::Null),
        ("after_signature", json!("")),
        ("after_signature", json!("a".repeat(64))),
    ] {
        let mut evidence = pixels();
        evidence[field] = bad;
        cases.push(json!({"postcondition":evidence}));
    }
    for result in cases {
        let before = result.clone();
        let state = DeliveryClaim::new("click", true, "ok", &result).state();
        assert_ne!(state, "delivered_and_pixel_verified", "{result}");
        assert_eq!(
            state,
            // #220 requires a final observed postcondition for general verification.
            // Invalid pixel signatures still cannot upgrade that claim to pixels.
            if result["postcondition"]["status"] == "observed_delta"
                && result["postcondition"]["observed_delta"] == true
            {
                "delivered_and_verified"
            } else {
                "delivered_unverified"
            }
        );
        assert_eq!(result, before, "classification must not mutate evidence");
    }
}
