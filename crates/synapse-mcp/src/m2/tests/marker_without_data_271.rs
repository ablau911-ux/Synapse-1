//! #271 (from #189): a delivery marker is evidence. An error that carries no
//! `data` object must still receive it, otherwise a possibly delivered click
//! is reported as an ordinary failure and gets retried.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::{ErrorData, model::ErrorCode};
use serde_json::json;

use super::{
    DELIVERED_NO_DELTA_RETRY_HINT, POSSIBLY_DELIVERED_RETRY_HINT, mark_input_delivered,
    mark_input_possibly_delivered,
};

fn error_with(data: Option<serde_json::Value>) -> ErrorData {
    ErrorData::new(ErrorCode(-32099), "failure reported without evidence", data)
}

#[test]
fn must_fail_delivered_marker_is_never_dropped_for_an_error_without_data() {
    let marked = mark_input_delivered(error_with(None));
    let data = marked
        .data
        .expect("#189: the marker must create the data object");
    assert_eq!(data["delivered"], true);
    assert_eq!(data["retry_hint"], DELIVERED_NO_DELTA_RETRY_HINT);
}

#[test]
fn must_fail_possibly_delivered_marker_is_never_dropped_for_an_error_without_data() {
    let marked = mark_input_possibly_delivered(error_with(None));
    let data = marked
        .data
        .expect("#189: the marker must create the data object");
    assert_eq!(data["possibly_delivered"], true);
    assert_eq!(data["retry_hint"], POSSIBLY_DELIVERED_RETRY_HINT);
    assert_ne!(
        data["delivered"], true,
        "unknown delivery never claims a delivery nobody observed"
    );
}

#[test]
fn must_pass_markers_keep_the_fields_an_error_already_carries() {
    let existing = || error_with(Some(json!({"code": "ACTION_TARGET_INVALID", "extra": 7})));
    let delivered = mark_input_delivered(existing()).data.unwrap();
    assert_eq!(delivered["code"], "ACTION_TARGET_INVALID");
    assert_eq!(delivered["extra"], 7);
    assert_eq!(delivered["delivered"], true);
    let possible = mark_input_possibly_delivered(existing()).data.unwrap();
    assert_eq!(possible["code"], "ACTION_TARGET_INVALID");
    assert_eq!(possible["extra"], 7);
    assert_eq!(possible["possibly_delivered"], true);
}
