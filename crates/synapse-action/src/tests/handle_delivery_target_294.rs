//! #294: every enqueue edge carries the handle's delivery window; releases
//! carry none and keep their queue lane.
use std::time::Duration;

use synapse_core::{Action, Backend, ButtonAction, MouseButton};

use super::ActionHandle;
use crate::{ActionError, InputDeliveryTarget};

const QUEUE_WAIT: Duration = Duration::from_secs(5);

fn target() -> InputDeliveryTarget {
    InputDeliveryTarget {
        root_hwnd: 0x11,
        pid: 101,
        source: "test_294",
    }
}

fn button(action: ButtonAction) -> Action {
    Action::MouseButton {
        button: MouseButton::Left,
        action,
        hold_ms: 0,
        backend: Backend::Software,
    }
}

fn missing(detail: &str) -> ActionError {
    ActionError::BackendUnavailable {
        detail: detail.to_owned(),
    }
}

/// The reflex/routine path and the #188 routine guard enqueue through
/// `try_execute*`; those actions must carry the same target as `execute`.
#[test]
fn must_pass_try_execute_carries_the_handle_delivery_target() -> Result<(), ActionError> {
    let (handle, mut rx) = ActionHandle::channel();
    let completion = handle
        .with_delivery_target(Some(target()))
        .try_execute_with_completion(button(ButtonAction::Down))?;
    let (_action, mut ack) = rx
        .try_recv()
        .map_err(|error| missing(&format!("queued action expected: {error}")))?;
    assert_eq!(ack.take_delivery_target(), Some(target()));
    drop(completion);
    Ok(())
}

#[test]
fn must_fail_untargeted_try_execute_carries_no_target() -> Result<(), ActionError> {
    let (handle, mut rx) = ActionHandle::channel();
    handle.try_execute(button(ButtonAction::Down))?;
    let (_action, mut ack) = rx
        .try_recv()
        .map_err(|error| missing(&format!("queued action expected: {error}")))?;
    assert_eq!(
        ack.take_delivery_target(),
        None,
        "no target must stay no target, so the backend guard fails closed"
    );
    Ok(())
}

/// A button-up stays on the ordered queue (it must not overtake a queued drag
/// move) and needs no target lookup to be released.
#[tokio::test]
async fn must_pass_button_up_keeps_queue_order_and_carries_no_target() -> Result<(), ActionError> {
    let (handle, mut rx, mut safety_rx) = ActionHandle::channel_with_safety_lane();
    let handle = handle.with_delivery_target(Some(target()));
    let task = tokio::spawn(async move { handle.execute(button(ButtonAction::Up)).await });
    let received = tokio::time::timeout(QUEUE_WAIT, rx.recv())
        .await
        .map_err(|_elapsed| missing("button-up never reached the ordered queue"))?;
    let (action, mut ack) = received.ok_or_else(|| missing("ordered queue closed"))?;
    assert!(matches!(
        action,
        Action::MouseButton {
            action: ButtonAction::Up,
            ..
        }
    ));
    assert!(
        safety_rx.try_recv().is_err(),
        "button-up must not jump the queue on the safety lane"
    );
    assert_eq!(
        ack.take_delivery_target(),
        None,
        "a release carries no target"
    );
    assert!(ack.send(Ok(())).is_ok());
    task.await
        .map_err(|error| missing(&format!("execute task failed: {error}")))?
}
