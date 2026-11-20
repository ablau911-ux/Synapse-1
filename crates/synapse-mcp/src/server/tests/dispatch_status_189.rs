//! #189: assert real target_act classification, not an invented test status.
use super::*;

pub(crate) fn assert_verify_needed(error: &ErrorData) {
    let status = target_act_error_status(error);
    assert_eq!(
        status, "verify_needed",
        "#189: possible delivery requires observation before retry: {error:?}"
    );
    assert_eq!(
        DeliveryClaim::without_result(false, status).state(),
        "delivered_unverified"
    );
}

/// #189: a failure proven to precede dispatch must never claim possible delivery.
pub(crate) fn assert_not_delivered(error: &ErrorData, expected_state: &str) {
    let status = target_act_error_status(error);
    assert_ne!(
        status, "verify_needed",
        "#189: proven pre-dispatch failure must not ask for re-observation: {error:?}"
    );
    assert_eq!(
        DeliveryClaim::without_result(false, status).state(),
        expected_state,
        "{error:?}"
    );
    let data = error.data.as_ref().expect("structured error data");
    assert_ne!(data["possibly_delivered"], true, "{error:?}");
    assert_ne!(data["delivered"], true, "{error:?}");
    assert_eq!(data["dispatch_not_started"], true, "{error:?}");
}
