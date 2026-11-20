use serde_json::json;

use super::{
    decoded_frame_dimensions, endpoint_safe_seek_time, sample_timestamp, validate_motion_range,
    validate_rgba_len, validate_sampling_range, video_origin,
};

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

fn assert_params_invalid(error: &rmcp::ErrorData) {
    assert_eq!(
        error_code(error),
        Some(synapse_core::error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
}

fn metadata(duration: &str) -> serde_json::Value {
    json!({
        "format": {
            "duration": duration,
            "start_time": "0.000000"
        },
        "streams": [{
            "duration": duration,
            "codec_type": "video"
        }]
    })
}

#[test]
fn must_pass_text_timeline_sampling_accepts_wider_count_than_image_analysis() {
    let parsed = validate_sampling_range(
        &metadata("10.0"),
        Some(1.0),
        Some(9.0),
        Some(24),
        Some(512),
        24,
    )
    .expect("text timeline should accept 24 samples");
    assert_eq!(parsed, (1.0, 9.0, 24, 512));

    let rejected = validate_sampling_range(
        &metadata("10.0"),
        Some(1.0),
        Some(9.0),
        Some(25),
        Some(512),
        24,
    )
    .expect_err("text timeline should keep a bounded sample count");
    assert_params_invalid(&rejected);
    assert!(
        rejected.message.contains("frame_count 1..=24"),
        "{rejected:?}"
    );
}

#[test]
fn must_pass_sample_timestamps_include_start_and_endpoint() {
    assert_eq!(sample_timestamp(0, 4, 2.0, 8.0), 2.0);
    assert_eq!(sample_timestamp(3, 4, 2.0, 8.0), 8.0);
    assert_eq!(sample_timestamp(0, 1, 2.0, 8.0), 2.0);
    assert_eq!(endpoint_safe_seek_time(3, 4, 8.0, 2.0), 7.999999);
    assert_eq!(endpoint_safe_seek_time(0, 4, 2.0, 2.0), 2.0);
}

#[test]
fn must_pass_decoded_dimensions_parse_showinfo_size() {
    let stderr = "[Parsed_showinfo_1 @ 000001] n:   0 pts:500000 pts_time:0.5 pos:-1 fmt:rgba sar:1/1 s:1000x750 i:P iskey:1 type:I";
    assert_eq!(
        decoded_frame_dimensions(stderr).expect("dimensions"),
        (1000, 750)
    );
}

#[test]
fn must_fail_raw_frame_validation_rejects_short_buffers() {
    validate_rgba_len(2, 2, &[0; 16]).expect("valid rgba");
    let err = validate_rgba_len(2, 2, &[0; 15]).expect_err("short rgba");
    assert_params_invalid(&err);
    assert!(err.message.contains("byte length mismatch"), "{err:?}");
}

#[test]
fn must_fail_video_origin_rejects_missing_start_time() {
    assert_eq!(video_origin(&metadata("1.0")).expect("origin"), 0.0);
    let err = video_origin(&json!({"format": {}, "streams": []})).expect_err("missing origin");
    assert_params_invalid(&err);
    assert!(err.message.contains("start timestamp"), "{err:?}");
}

#[test]
fn must_fail_sampling_errors_report_effective_values_and_probed_duration() {
    for (start, end, count, size) in [
        (0.0, 8.0, 6, 1280),
        (7.5, 7.5, 6, 1280),
        (-1.0, 7.5, 6, 1280),
        (0.0, 7.5, 0, 127),
        (f64::NAN, 7.5, 6, 1280),
    ] {
        let Err(error) = validate_sampling_range(
            &metadata("7.5"),
            Some(start),
            Some(end),
            Some(count),
            Some(size),
            12,
        ) else {
            panic!("invalid sampling parameters accepted");
        };
        assert_params_invalid(&error);
        for field in [
            format!("start_seconds={start}"),
            format!("end_seconds={end}"),
            "duration_seconds=7.5".to_owned(),
            format!("frame_count={count}"),
            format!("max_long_edge={size}"),
        ] {
            assert!(error.message.contains(&field), "{error:?} missing {field}");
        }
    }
    assert!(matches!(
        validate_sampling_range(&metadata("7.5"), None, None, None, None, 12),
        Ok((0.0, 7.5, 6, 1280))
    ));
}

#[test]
fn must_fail_motion_errors_report_effective_values_and_probed_duration() {
    for (start, end) in [(0.0, 10.0), (9.8, 9.8), (-1.0, 9.8), (0.0, f64::INFINITY)] {
        let Err(error) = validate_motion_range(&metadata("9.8"), Some(start), Some(end)) else {
            panic!("invalid motion range accepted");
        };
        assert_params_invalid(&error);
        assert!(
            error.message.contains(&format!("start_seconds={start}")),
            "{error:?}"
        );
        assert!(
            error.message.contains(&format!("end_seconds={end}")),
            "{error:?}"
        );
        assert!(error.message.contains("duration_seconds=9.8"), "{error:?}");
    }
    assert!(matches!(
        validate_motion_range(&metadata("9.8"), None, None),
        Ok((0.0, 9.8))
    ));
    let Err(error) = validate_motion_range(&metadata("120"), Some(0.0), Some(61.0)) else {
        panic!("oversized motion range accepted");
    };
    assert_params_invalid(&error);
    assert!(error.message.contains("range_seconds=61"), "{error:?}");
}
