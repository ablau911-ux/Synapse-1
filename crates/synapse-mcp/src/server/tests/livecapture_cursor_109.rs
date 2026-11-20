//! Regression tests for #109: `livecapture next.after_frame` accepts the native
//! cursor object or a JSON-string encoding of it, drops echoed extra fields, and
//! still rejects malformed or structurally invalid cursors.

use synapse_core::CaptureFrameCursor;

use super::LiveCaptureParams;

/// The cursor every fixture in this file encodes.
fn expected_cursor() -> CaptureFrameCursor {
    CaptureFrameCursor {
        capture_id: "capture".to_owned(),
        generation: 1,
        frame_seq: 2,
    }
}

/// Parses `params` as `operation=next` and returns (window_hwnd, after_frame).
fn parse_next(params: serde_json::Value) -> (i64, CaptureFrameCursor) {
    match serde_json::from_value::<LiveCaptureParams>(params) {
        Ok(LiveCaptureParams::Next {
            window_hwnd,
            after_frame,
            ..
        }) => (window_hwnd, after_frame),
        Ok(other) => panic!("operation=next must parse as Next, got {other:?}"),
        Err(error) => panic!("operation=next must deserialize: {error}"),
    }
}

#[test]
fn must_pass_livecapture_next_accepts_native_cursor_object() {
    let cursor = serde_json::json!({"capture_id":"capture", "generation":1, "frame_seq":2});
    let params = serde_json::json!({"operation":"next", "window_hwnd":123, "after_frame":cursor});
    assert_eq!(parse_next(params), (123, expected_cursor()));
}

#[test]
fn must_pass_livecapture_next_accepts_json_string_cursor() {
    let params = serde_json::json!({
        "operation":"next",
        "window_hwnd":123,
        "after_frame":"{\"capture_id\":\"capture\",\"generation\":1,\"frame_seq\":2}"
    });
    assert_eq!(parse_next(params), (123, expected_cursor()));
}

#[test]
fn must_pass_livecapture_next_accepts_echoed_cursor_with_extra_fields() {
    // Real clients echo back the richer cursor-bearing frame object this
    // tool itself emits (dirty_region, frame_reused, ...); the essential
    // fields must win and the extras must be dropped, in BOTH forms.
    let object_form = serde_json::json!({
        "operation":"next", "window_hwnd":123,
        "after_frame":{
            "capture_id":"capture", "generation":1, "frame_seq":2,
            "dirty_region":{"x":0,"y":0,"w":442,"h":148},
            "frame_reused":false, "image_sha256":"ab", "sample_age_ms":4
        }
    });
    assert_eq!(parse_next(object_form), (123, expected_cursor()));

    let string_form = serde_json::json!({
        "operation":"next", "window_hwnd":123,
        "after_frame":"{\"capture_id\":\"capture\",\"generation\":1,\"frame_seq\":2,\"frame_reused\":true,\"dirty_region\":{\"x\":0}}"
    });
    assert_eq!(parse_next(string_form), (123, expected_cursor()));
}

#[test]
fn must_fail_livecapture_next_rejects_echoed_cursor_with_wrong_typed_field() {
    let params = serde_json::json!({
        "operation":"next", "window_hwnd":123,
        "after_frame":{"capture_id":"capture", "generation":"one", "frame_seq":2,
                       "frame_reused":false}
    });
    let error = serde_json::from_value::<LiveCaptureParams>(params)
        .expect_err("wrong-typed essential field must fail even with extras present");
    assert!(
        error.to_string().contains("invalid type: string"),
        "unexpected error: {error}"
    );
}

#[test]
fn must_fail_livecapture_next_rejects_malformed_json_string_cursor() {
    let params = serde_json::json!({
        "operation":"next", "window_hwnd":123, "after_frame":"{not-json}"
    });
    let error = serde_json::from_value::<LiveCaptureParams>(params)
        .expect_err("malformed JSON-string cursor must fail");
    assert!(
        error
            .to_string()
            .contains("after_frame JSON string must encode a valid capture cursor object"),
        "unexpected error: {error}"
    );
}

#[test]
fn must_fail_livecapture_next_rejects_structurally_invalid_cursor() {
    let params = serde_json::json!({
        "operation":"next",
        "window_hwnd":123,
        "after_frame":{"capture_id":"capture", "generation":1}
    });
    let error = serde_json::from_value::<LiveCaptureParams>(params)
        .expect_err("cursor missing frame_seq must fail");
    assert!(
        error.to_string().contains("missing field `frame_seq`"),
        "unexpected error: {error}"
    );
}
