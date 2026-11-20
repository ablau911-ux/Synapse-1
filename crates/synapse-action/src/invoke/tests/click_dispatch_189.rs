use super::*;
use crate::ActionError;

#[test]
fn must_fail_dispatch_stage_survives_stale_and_provider_error_mapping() {
    let id = synapse_core::element_id(189, "01020304");
    for delivered in [false, true] {
        for stale in [false, true] {
            let source = if stale {
                synapse_a11y::A11yError::ElementStale {
                    detail: "element not available".to_owned(),
                }
            } else {
                synapse_a11y::A11yError::internal("arbitrary provider error")
            };
            let error = a11y_error_to_action(
                &id,
                synapse_a11y::A11yError::ClickDispatch {
                    source: Box::new(source),
                    delivered,
                },
            );
            assert!(
                matches!(&error, ActionError::ClickDispatch { delivered: actual, .. } if *actual == delivered)
            );
            assert_eq!(
                error.code(),
                if stale {
                    synapse_core::error_codes::TRANSIENT_ELEMENT_EXPIRED
                } else {
                    synapse_core::error_codes::ACTION_TARGET_INVALID
                }
            );
            let updated = error.with_detail("annotated");
            assert_eq!(updated.detail(), "annotated");
            assert!(
                matches!(updated, ActionError::ClickDispatch { delivered: actual, .. } if actual == delivered)
            );
        }
    }
}

/// #189 U9/U10: only a UIA worker admission refusal marks the tier unavailable.
#[test]
fn must_fail_not_dispatched_mapping_keeps_evidence_and_admission_cause() {
    use synapse_a11y::{A11yError, UiaWorkerPhase};
    let id = synapse_core::element_id(189, "01020304");
    for (source, tier_unavailable) in [
        (
            A11yError::uia_worker_timeout(UiaWorkerPhase::WorkerBusy, "busy"),
            true,
        ),
        (
            A11yError::uia_worker_timeout(UiaWorkerPhase::WorkerLimit, "limit"),
            true,
        ),
        (
            A11yError::uia_worker_timeout(UiaWorkerPhase::WorkerUnavailable, "retired"),
            true,
        ),
        (A11yError::internal("send UIA worker job failed"), false),
        (
            A11yError::ElementStale {
                detail: "element not available".to_owned(),
            },
            false,
        ),
    ] {
        let error = a11y_error_to_action(
            &id,
            A11yError::ClickNotDispatched {
                source: Box::new(source),
            },
        );
        assert!(
            matches!(&error, ActionError::ClickNotDispatched { tier_unavailable: actual, .. } if *actual == tier_unavailable),
            "{error:?}"
        );
    }
}
