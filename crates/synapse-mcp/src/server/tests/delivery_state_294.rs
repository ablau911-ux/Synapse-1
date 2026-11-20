//! #294: OS-input refusals by the delivery guard reach the client as
//! `delivery_state=refused_before_delivery`; unproven ones stay unverified.
use super::*;

fn delivery_state(error: &ErrorData) -> &'static str {
    DeliveryClaim::without_result(false, target_act_error_status(error)).state()
}

#[test]
fn must_pass_input_target_required_is_refused_before_delivery() {
    let error = crate::m2::action_error_to_mcp(&synapse_action::ActionError::InputTargetRequired {
        detail: "no intended target window".to_owned(),
    });
    assert_eq!(
        target_act_error_code(&error),
        Some(error_codes::ACTION_INPUT_TARGET_REQUIRED)
    );
    assert_eq!(delivery_state(&error), "refused_before_delivery");
}

#[test]
fn must_pass_guard_refusal_proven_not_started_is_refused_before_delivery() {
    let error = crate::m2::action_error_to_mcp(&synapse_action::ActionError::ClickNotDispatched {
        source: Box::new(synapse_action::ActionError::ForegroundLost {
            detail: "target is not the live foreground; the batch was not sent".to_owned(),
        }),
        tier_unavailable: false,
    });
    assert_eq!(
        target_act_error_code(&error),
        Some(error_codes::ACTION_FOREGROUND_LOST)
    );
    assert_eq!(delivery_state(&error), "refused_before_delivery");
}

#[test]
fn must_fail_foreground_lost_without_dispatch_proof_stays_unverified() {
    let error = crate::m2::action_error_to_mcp(&synapse_action::ActionError::ForegroundLost {
        detail: "foreground left the target mid-run".to_owned(),
    });
    assert_eq!(
        delivery_state(&error),
        "delivered_unverified",
        "a mid-run foreground loss may have delivered input and must be re-observed"
    );
}
