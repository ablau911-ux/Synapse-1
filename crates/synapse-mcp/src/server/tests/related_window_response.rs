//! #92: a verified related-window transition must reach the public facade.
use super::*;
use crate::m2::postcondition::{postcondition_not_requested, postcondition_observed_delta};

#[test]
fn must_pass_related_window_response_preserves_verified_and_unverified_outcomes()
-> Result<(), ErrorData> {
    for verified in [false, true] {
        let mut postcondition = if verified {
            postcondition_observed_delta(
                "act_press",
                "foreground_related_window",
                "before".into(),
                "after".into(),
                "related_window_observed after delivery; causality not established",
            )
        } else {
            postcondition_not_requested("act_press", "foreground_related_window")
        };
        if verified {
            postcondition.related_window_observed =
                Some(crate::m2::postcondition::RelatedWindowObservation {
                    hwnd: 200,
                    pid: 10,
                    title: "DevTools".into(),
                    target_hwnd: 100,
                    relationship: "same_process_owner_chain".into(),
                });
        }
        let response = crate::m2::ActPressResponse {
            ok: true,
            keys_pressed: 3,
            elapsed_ms: 250,
            backend_used: "software".into(),
            backend_tier_used: "foreground".into(),
            required_foreground: true,
            postcondition,
        };
        let (_, ok, status, result) = target_act_foreground_key_response(Ok(Json(response)))?;
        assert_eq!(ok, verified);
        if verified {
            assert_eq!(
                result["postcondition"]["related_window_observed"]["hwnd"],
                200
            );
            assert_eq!(
                result["postcondition"]["related_window_observed"]["pid"],
                10
            );
            assert_eq!(
                result["postcondition"]["related_window_observed"]["relationship"],
                "same_process_owner_chain"
            );
        } else {
            assert!(
                result["postcondition"]
                    .get("related_window_observed")
                    .is_none()
            );
        }
        assert_eq!(
            DeliveryClaim::new("key", ok, status, &result).state(),
            if verified {
                "delivered_and_verified"
            } else {
                "delivered_unverified"
            },
        );
    }
    Ok(())
}

#[test]
fn must_fail_related_window_response_unrelated_foreground_remains_error() -> Result<(), ErrorData> {
    let error = mcp_error(
        error_codes::ACTION_FOREGROUND_LOST,
        "unexpected foreground change",
    );
    let (delegated, ok, status, result) = target_act_foreground_key_response(Err(error))?;
    assert_eq!(delegated, "act_press");
    assert!(!ok);
    assert_eq!(status, "verify_needed");
    assert_eq!(
        DeliveryClaim::new("key", ok, status, &result).state(),
        "delivered_unverified"
    );
    assert_eq!(result["error"]["code"], "ACTION_FOREGROUND_LOST");
    assert_eq!(result["error"]["delegated_tool"], "act_press");
    assert!(result.get("postcondition").is_none());
    Ok(())
}
