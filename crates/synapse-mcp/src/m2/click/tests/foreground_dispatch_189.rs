use super::*;
use std::sync::{Arc, Mutex};

fn button() -> Action {
    Action::MouseButton {
        button: synapse_core::MouseButton::Left,
        action: synapse_core::ButtonAction::Press,
        hold_ms: 1,
        backend: synapse_core::Backend::Software,
    }
}

#[tokio::test]
async fn must_fail_foreground_reply_error_retains_possible_delivery() {
    let deliveries = Arc::new(Mutex::new(0));
    let counted = deliveries.clone();
    let result = execute_actor_actions_with(
        vec![button()],
        synapse_action::cached_double_click_timing(),
        move |_| {
            *counted.lock().unwrap() += 1;
            async {
                Err(synapse_action::ActionError::BackendUnavailable {
                    detail: "reply lost after button".to_owned(),
                })
            }
        },
    )
    .await;
    assert_eq!(*deliveries.lock().unwrap(), 1);
    assert_eq!(
        result.unwrap_err().data.unwrap()["possibly_delivered"],
        true
    );
}

#[tokio::test]
async fn must_fail_foreground_second_click_error_retains_first_delivery() {
    let deliveries = Arc::new(Mutex::new(0));
    let counted = deliveries.clone();
    let mut calls = 0;
    let result = execute_actor_actions_with(
        vec![button(), button()],
        synapse_action::cached_double_click_timing(),
        move |_| {
            calls += 1;
            let result = if calls == 1 {
                *counted.lock().unwrap() += 1;
                Ok(())
            } else {
                Err(synapse_action::ActionError::TargetInvalid {
                    detail: "second click refused".to_owned(),
                })
            };
            async move { result }
        },
    )
    .await;
    assert_eq!(*deliveries.lock().unwrap(), 1);
    assert_eq!(result.unwrap_err().data.unwrap()["delivered"], true);
}

/// F4: a MouseMove that fails before any button event is proven undelivered.
#[tokio::test]
async fn must_fail_foreground_move_failure_before_any_button_is_not_started() {
    let buttons = Arc::new(Mutex::new(0));
    let counted = buttons.clone();
    let movement = Action::MouseMove {
        to: MouseTarget::Screen {
            point: synapse_core::Point { x: 20, y: 20 },
        },
        curve: AimCurve::Instant,
        duration_ms: 0,
        backend: synapse_core::Backend::Software,
    };
    let result = execute_actor_actions_with(
        vec![movement, button()],
        synapse_action::cached_double_click_timing(),
        move |action| {
            if matches!(action, Action::MouseButton { .. }) {
                *counted.lock().unwrap() += 1;
            }
            async {
                Err(synapse_action::ActionError::BackendUnavailable {
                    detail: "cursor move rejected".to_owned(),
                })
            }
        },
    )
    .await;
    assert_eq!(*buttons.lock().unwrap(), 0);
    let error = result.unwrap_err();
    crate::server::dispatch_status_189_tests::assert_not_delivered(
        &error,
        "failed_before_delivery",
    );
}

/// F2: a lease held by another session refuses SendInput before any input.
#[test]
fn must_fail_foreground_lease_busy_is_refused_before_delivery() {
    let _serial = crate::test_support::lease_serial("lease_busy_189");
    let _holder = crate::m2::acquire_foreground_input_lease_with_ttl(
        "act_click",
        Some("holder-189"),
        synapse_action::DEFAULT_LEASE_TTL_MS,
    )
    .unwrap();
    let policy = super::super::ForegroundClickPolicy::allowed(Some("requester-189".to_owned()));
    let mut attempts = Vec::new();
    let error = super::super::acquire_click_foreground_lease(&policy, 1, &mut attempts, None)
        .err()
        .unwrap();
    assert_eq!(
        error.data.as_ref().unwrap()["code"],
        synapse_core::error_codes::ACTION_FOREGROUND_LEASE_BUSY
    );
    crate::server::dispatch_status_189_tests::assert_not_delivered(
        &error,
        "refused_before_delivery",
    );
}
