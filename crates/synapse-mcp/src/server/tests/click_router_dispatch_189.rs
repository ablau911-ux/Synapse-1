//! #189: producer -> real verified router -> target_act classification.
//! Inputs use the typed shapes the UIA producer emits (resolve_click.rs
//! `click_dispatch_evidence`, mapped by synapse-action `a11y_error_to_action`).
use super::*;
use crate::m2::dispatch_189_test_support::after_dispatch;
use crate::server::background_router::dispatch_status_189_tests::assert_verify_needed;
use synapse_action::ActionError;

async fn terminal(error: ErrorData, delivered: bool) {
    let port = Port::new(error, Ok(observed()));
    let result = route(&port, params()).await;
    assert_eq!(
        port.calls(),
        ["first"],
        "#189: never re-click after dispatch"
    );
    let error = result
        .err()
        .unwrap_or_else(|| panic!("delivery must remain unverified"));
    assert_verify_needed(&error);
    let data = error
        .data
        .as_ref()
        .unwrap_or_else(|| panic!("delivery evidence required"));
    if delivered {
        assert_eq!(data["delivered"], true);
    } else {
        assert_eq!(data["possibly_delivered"], true);
        assert_ne!(
            data["delivered"], true,
            "unknown delivery must not claim certainty"
        );
    }
}

/// A sent job that never answered: typed "dispatched, delivery unknown".
fn sent_without_reply(source: ActionError) -> ActionError {
    ActionError::ClickDispatch {
        source: Box::new(source),
        delivered: false,
    }
}

#[tokio::test]
async fn must_fail_worker_reply_timeout_never_sends_second_click() {
    let error = after_dispatch(
        &sent_without_reply(ActionError::BackendUnavailable {
            detail: "operation=element_click phase=worker_reply_recv timeout_ms=250".to_owned(),
        }),
        0,
        1,
    );
    terminal(error, false).await;
}

#[tokio::test]
async fn must_fail_click_two_error_preserves_first_delivery_without_replay() {
    for source in [
        ActionError::ElementPatternUnsupported {
            element_id: synapse_core::element_id(189, "01020304"),
            detail: "click 2 unsupported after click 1 Invoke".to_owned(),
        },
        sent_without_reply(ActionError::BackendUnavailable {
            detail: "phase=worker_reply_recv".to_owned(),
        }),
        ActionError::TargetInvalid {
            detail: "click 2 target closed after click 1 Invoke".to_owned(),
        },
    ] {
        let error = after_dispatch(&source, 1, 2);
        let attempts = click_tier_attempts_from_error(&error);
        assert!(
            attempts
                .iter()
                .any(|a| a.status == "delivered" && a.tier == "uia")
        );
        terminal(error, true).await;
    }
}

#[tokio::test]
async fn must_fail_element_closed_during_invoke_is_verify_needed_without_reclick() {
    for source in [
        ActionError::TransientElementExpired {
            element_id: synapse_core::element_id(189, "01020304"),
            detail: "InvokePattern.invoke failed: element closed during Invoke".to_owned(),
        },
        ActionError::TargetInvalid {
            detail: "InvokePattern.invoke failed: element closed during Invoke".to_owned(),
        },
    ] {
        terminal(after_dispatch(&sent_without_reply(source), 0, 1), false).await;
    }
}

#[tokio::test]
async fn must_fail_worker_reply_disconnect_is_verify_needed_without_reclick() {
    let source = sent_without_reply(ActionError::TargetInvalid {
        detail: "receive UIA worker job failed for operation=element_click: channel disconnected"
            .to_owned(),
    });
    terminal(after_dispatch(&source, 0, 1), false).await;
}
