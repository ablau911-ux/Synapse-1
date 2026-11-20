//! #189: real PostMessage error producer; source guard covers loop ownership.
use super::*;
use serde_json::json;

pub(crate) fn produce(error: &synapse_action::ActionError, button_posted: bool) -> ErrorData {
    posted_click_error(error, button_posted)
}

#[test]
fn must_fail_postmessage_after_down_is_delivered_and_terminal() {
    let source = synapse_action::ActionError::BackendUnavailable {
        detail: "PostMessageW failed while posting Up after successful Down".to_owned(),
    };
    let error = produce(&source, true);
    let data = error
        .data
        .unwrap_or_else(|| panic!("delivery data required"));
    assert_eq!(data["delivered"], true);
    assert_eq!(data["router_escalation_allowed"], false);
    assert_eq!(
        data["router_stop_reason"],
        "postmessage_partial_click_delivered"
    );
    let unsent = produce(&source, false);
    assert_ne!(
        unsent.data.as_ref().map(|data| &data["delivered"]),
        Some(&json!(true))
    );
}

fn params(clicks: u8) -> ActClickParams {
    serde_json::from_value(serde_json::json!({
        "target": {"x": 20, "y": 20}, "clicks": clicks, "hold_ms": 1
    }))
    .unwrap()
}

fn timing() -> DoubleClickTiming {
    DoubleClickTiming {
        window_ms: 500,
        inter_click_delay_ms: 0,
        source: "test",
    }
}

#[tokio::test]
async fn must_fail_real_post_loop_stops_after_first_button_on_identity_or_post_failure() {
    use std::sync::{Arc, Mutex};
    for fail_identity in [false, true] {
        for fail_at in [2, 3] {
            let delivered = Arc::new(Mutex::new(0));
            let observed = delivered.clone();
            let mut checks = 0;
            let mut posts = 0;
            let result = post_hwnd_clicks_with(
                &params(2),
                timing(),
                true,
                || {
                    let call = checks;
                    checks += 1;
                    if fail_identity && call == fail_at {
                        Err(synapse_action::ActionError::TargetInvalid {
                            detail: "child disappeared".to_owned(),
                        })
                    } else {
                        Ok(())
                    }
                },
                |step| {
                    let call = posts;
                    posts += 1;
                    if !fail_identity && call == fail_at {
                        return Err(synapse_action::ActionError::BackendUnavailable {
                            detail: "post failed".to_owned(),
                        });
                    }
                    if matches!(step, PostedMouseStep::Down | PostedMouseStep::DoubleClick) {
                        *observed.lock().unwrap() += 1;
                    }
                    Ok(())
                },
            )
            .await;
            assert_eq!(
                *delivered.lock().unwrap(),
                1,
                "control-189: never replay a completed/down click"
            );
            let data = result.expect_err("partial sequence").data.unwrap();
            assert_eq!(data["delivered"], true);
            assert_eq!(data["router_escalation_allowed"], false);
        }
    }
}

#[tokio::test]
async fn must_pass_post_failure_before_down_proves_no_control_delivery() {
    let mut delivered = 0;
    let error = post_hwnd_clicks_with(
        &params(1),
        timing(),
        false,
        || Ok(()),
        |step| {
            if step == PostedMouseStep::Down {
                return Err(synapse_action::ActionError::BackendUnavailable {
                    detail: "first down refused".to_owned(),
                });
            }
            if step == PostedMouseStep::Up {
                delivered += 1;
            }
            Ok(())
        },
    )
    .await
    .expect_err("no button queued");
    assert_eq!(delivered, 0);
    assert_eq!(error.data.as_ref().unwrap()["dispatch_not_started"], true);
    assert_ne!(error.data.as_ref().unwrap()["delivered"], true);
}
