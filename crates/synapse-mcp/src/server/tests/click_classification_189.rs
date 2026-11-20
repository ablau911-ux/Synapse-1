use super::super::verified_click_router::{carry_click_delivery, click_delivery_snapshot};
use super::*;
use crate::m2::dispatch_189_test_support::after_dispatch;
use crate::server::background_router::dispatch_status_189_tests::assert_verify_needed;

#[tokio::test]
async fn must_fail_each_postdispatch_pattern_error_is_verify_needed_without_native_delivery() {
    for delivered in [false, true] {
        for pattern in ["Invoke", "Toggle", "Select", "Expand", "Collapse", "Legacy"] {
            let error = after_dispatch(
                &synapse_action::ActionError::ClickDispatch {
                    delivered,
                    source: Box::new(synapse_action::ActionError::TargetInvalid {
                        detail: format!("{pattern}: arbitrary error"),
                    }),
                },
                0,
                1,
            );
            let port = Port::new(error, Err(no_delta()));
            let error = route(&port, params())
                .await
                .expect_err("requires observation");
            assert_eq!(
                port.calls(),
                if delivered {
                    vec!["first", "verify"]
                } else {
                    vec!["first"]
                }
            );
            assert_verify_needed(&error);
            assert_eq!(
                click_tier_attempts_from_error(&error)
                    .iter()
                    .filter(|a| a.status == "delivered")
                    .count(),
                usize::from(delivered)
            );
        }
    }
}

#[tokio::test]
async fn must_pass_returned_pattern_reconciles_observation_without_another_delivery() {
    let source = synapse_action::ActionError::ClickDispatch {
        delivered: true,
        source: Box::new(synapse_action::ActionError::TargetInvalid {
            detail: "state read failed".to_owned(),
        }),
    };
    let port = Port::new(after_dispatch(&source, 0, 1), Ok(observed()));
    let response = route(&port, params())
        .await
        .expect("observation reconciles delivered input");
    assert_eq!(port.calls(), ["first", "verify"]);
    assert_eq!(response.postcondition.status, "observed_delta");
}

#[test]
fn must_fail_postdelivery_observation_guard_or_audit_error_cannot_erase_attempts() {
    let source = include_str!("../m2_tools.rs");
    let after_dispatch = source
        .split_once("dispatched = Some(click_delivery_snapshot(&result));")
        .expect("live delivery snapshot")
        .1
        .split_once("result.map(Json)")
        .expect("click handler return")
        .0;
    assert!(
        !after_dispatch.contains("audit_try!"),
        "#189: post-dispatch early audit return can erase delivery if that audit also fails"
    );
    // #302/#318 (6490711f): one `carry_delivery` closure carries the delivery
    // snapshot into the action error and into the audit error alike.
    assert!(
        after_dispatch.contains("Some(dispatched) => carry_click_delivery(error, dispatched)"),
        "#189: the post-dispatch error carrier must attach the delivery snapshot"
    );
    assert!(
        after_dispatch.contains("let result = result.map_err(&carry_delivery);"),
        "#189: a post-dispatch action error must carry the delivery"
    );
    assert!(
        after_dispatch.contains(".map_err(carry_delivery)?;"),
        "#189: a post-dispatch audit error must carry the delivery"
    );
    let dispatched = click_delivery_snapshot(&Ok(delivered(CLICK_TIER_POSTMESSAGE)));
    for code in [
        error_codes::TOOL_INTERNAL_ERROR,
        error_codes::ACTION_TARGET_INVALID,
        error_codes::ACTION_BACKEND_UNAVAILABLE,
    ] {
        let error =
            carry_click_delivery(mcp_error(code, "followup failed after input"), &dispatched);
        assert_verify_needed(&error);
        assert_eq!(click_tier_attempts_from_error(&error).len(), 1);
    }
}

#[test]
fn must_fail_lost_reply_followup_never_claims_refusal_or_certain_delivery() {
    let source = after_dispatch(
        &synapse_action::ActionError::ClickDispatch {
            source: Box::new(synapse_action::ActionError::BackendUnavailable {
                detail: "phase=worker_reply_recv".to_owned(),
            }),
            delivered: false,
        },
        0,
        1,
    );
    let dispatched = click_delivery_snapshot(&Err(source));
    let error = carry_click_delivery(
        mcp_error(error_codes::ACTION_TARGET_INVALID, "later read failed"),
        &dispatched,
    );
    assert_verify_needed(&error);
    assert_eq!(error.data.as_ref().unwrap()["possibly_delivered"], true);
    assert_ne!(error.data.as_ref().unwrap()["delivered"], true);
}
