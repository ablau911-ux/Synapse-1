//! #113 motion must share sampling's finite stream-duration fallback.
use super::validate_motion_range;
use serde_json::json;

#[test]
fn must_pass_motion_accepts_stream_duration_when_format_is_missing_or_na() {
    for format in [
        json!({}),
        json!({"duration":"N/A"}),
        json!({"duration":"NaN"}),
    ] {
        let metadata = json!({"format":format,"streams":[{"duration":"12.5"}]});
        assert_eq!(
            validate_motion_range(&metadata, Some(2.5), None).expect("stream duration"),
            (2.5, 12.5)
        );
        assert_eq!(
            validate_motion_range(&metadata, Some(1.0), Some(3.0)).expect("explicit range"),
            (1.0, 3.0)
        );
    }
}

#[test]
fn must_pass_valid_format_duration_takes_precedence_over_stream_duration() {
    let metadata = json!({"format":{"duration":"9"},"streams":[{"duration":"12.5"}]});
    assert_eq!(
        validate_motion_range(&metadata, None, None).expect("format duration"),
        (0.0, 9.0)
    );
}

#[test]
fn must_fail_end_past_stream_duration_names_exact_values_without_clipping() {
    let metadata = json!({"format":{"duration":"N/A"},"streams":[{"duration":"12.5"}]});
    let before = metadata.clone();
    let error = validate_motion_range(&metadata, Some(2.5), Some(13.0)).expect_err("no clipping");
    assert_eq!(error.data.as_ref().unwrap()["code"], "TOOL_PARAMS_INVALID");
    for part in [
        "start_seconds=2.5",
        "end_seconds=13",
        "duration_seconds=12.5",
        "range_seconds=10.5",
    ] {
        assert!(error.message.contains(part), "{error}");
    }
    assert_eq!(metadata, before);
}

#[test]
fn must_fail_unavailable_duration_keeps_requested_range_in_diagnostic() {
    let metadata = json!({"format":{"duration":"N/A"},"streams":[{"duration":"0"}]});
    let error = validate_motion_range(&metadata, Some(2.0), Some(4.0)).expect_err("no duration");
    assert_eq!(error.data.as_ref().unwrap()["code"], "TOOL_PARAMS_INVALID");
    for part in [
        "start_seconds=Some(2.0)",
        "end_seconds=Some(4.0)",
        "duration_seconds=unavailable",
    ] {
        assert!(error.message.contains(part), "{error}");
    }
}
