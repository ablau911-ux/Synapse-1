use super::*;
use crate::UiaWorkerPhase;
use std::cell::Cell;

#[test]
fn must_fail_returned_pattern_readback_retains_exactly_one_control_delivery() {
    for pattern in ["Invoke", "Toggle", "Select", "Expand", "Collapse", "Legacy"] {
        let delivered = Cell::new(0);
        let result: A11yResult<()> = click_dispatch_with(
            || {
                delivered.set(delivered.get() + 1);
                Ok(())
            },
            || {
                Err(A11yError::internal(format!(
                    "{pattern}: arbitrary readback error"
                )))
            },
        );
        assert_eq!(delivered.get(), 1, "control-189/{pattern}");
        assert!(
            matches!(
                result,
                Err(A11yError::ClickDispatch {
                    delivered: true,
                    ..
                })
            ),
            "#189: successful mutation must preserve delivery even without known error text"
        );
    }
}

#[test]
fn must_fail_provider_error_is_possibly_delivered_without_readback_or_second_dispatch() {
    let delivered = Cell::new(0);
    let readbacks = Cell::new(0);
    let result: A11yResult<()> = click_dispatch_with(
        || {
            delivered.set(delivered.get() + 1);
            Err(A11yError::ElementStale {
                detail: "target closed during mutation".to_owned(),
            })
        },
        || {
            readbacks.set(readbacks.get() + 1);
            Ok(())
        },
    );
    assert_eq!((delivered.get(), readbacks.get()), (1, 0));
    assert!(matches!(
        result,
        Err(A11yError::ClickDispatch {
            delivered: false,
            ..
        })
    ));
}

#[test]
fn must_pass_successful_dispatch_keeps_readback_value_and_one_delivery() {
    let delivered = Cell::new(0);
    let result = click_dispatch_with(
        || {
            delivered.set(delivered.get() + 1);
            Ok(())
        },
        || Ok("selected"),
    );
    assert_eq!(result.unwrap(), "selected");
    assert_eq!(delivered.get(), 1);
}

#[test]
fn must_fail_native_pattern_variants_bypassing_dispatch_boundary() {
    let source = include_str!("../resolve_click.rs").replace("\r\n", "\n");
    let body_of = |function: &str| -> String {
        let start = source.find(&format!("fn {function}(")).unwrap();
        source[start..].split("\n}\n").next().unwrap().to_owned()
    };
    // #300 (291fbef4) moved the invoke and toggle dispatch into helpers; the
    // pattern functions must reach the pattern call only through them.
    for (caller, helper, direct_call) in [
        ("try_invoke_pattern", "invoke_pattern_action(", ".invoke()"),
        (
            "toggle_pattern_action",
            "toggle_and_read_state(",
            ".toggle()",
        ),
    ] {
        let body = body_of(caller);
        assert!(
            body.contains(helper) && !body.contains(direct_call),
            "{caller} bypasses its dispatch helper {helper}"
        );
    }
    for function in [
        "invoke_pattern_action",
        "toggle_and_read_state",
        "selection_item_pattern_action",
        "expand_pattern_action",
        "collapse_pattern_action",
        "legacy_default_action",
    ] {
        let body = body_of(function);
        assert!(
            body.contains("click_dispatch_with("),
            "{function} lost the typed dispatch boundary"
        );
    }
}

/// U3/U8/U10: failures before any pattern call are typed "not dispatched".
#[test]
fn must_fail_before_pattern_failures_are_typed_not_dispatched() {
    for error in [
        A11yError::ElementStale {
            detail: "re-resolve found no element".to_owned(),
        },
        A11yError::internal("TogglePattern ToggleState before Toggle failed"),
        A11yError::internal("send UIA worker job failed: receiver closed"),
    ] {
        assert!(
            matches!(
                click_dispatch_evidence(error),
                A11yError::ClickNotDispatched { .. }
            ),
            "#189: a pre-pattern failure is proven undelivered"
        );
    }
}

/// U9: admission refusals are typed by phase and stay "not dispatched".
#[test]
fn must_pass_worker_admission_phases_stay_not_dispatched() {
    for phase in [
        UiaWorkerPhase::WorkerLimit,
        UiaWorkerPhase::WorkerBusy,
        UiaWorkerPhase::WorkerUnavailable,
    ] {
        let typed = click_dispatch_evidence(A11yError::uia_worker_timeout(phase, "admission"));
        assert!(
            matches!(&typed, A11yError::ClickNotDispatched { source }
                if matches!(**source, A11yError::UiaWorkerTimeout { phase: actual, .. } if actual == phase)),
            "{phase:?}: {typed:?}"
        );
    }
}

/// U13/U14: a job the worker received but never answered may have dispatched.
#[test]
fn must_fail_sent_job_without_reply_is_typed_possibly_dispatched() {
    for error in [
        A11yError::uia_worker_timeout(UiaWorkerPhase::ReplyRecv, "reply deadline"),
        A11yError::UiaWorkerReplyLost {
            detail: "reply channel disconnected".to_owned(),
        },
    ] {
        assert!(matches!(
            click_dispatch_evidence(error),
            A11yError::ClickDispatch {
                delivered: false,
                ..
            }
        ));
    }
}
