//! Regression tests for #109: `livecapture` `next.after_frame` must satisfy
//! strict client-side `inputSchema` validation (`SchemaValidator`) as well as
//! runtime deserialization for echoed frame objects and JSON-stringified
//! cursors, reject non-object JSON strings, and dispatch through `SynapseService`.

use std::time::Duration;

use anyhow::{Context, ensure};
use rmcp::model::Tool;
use serde_json::{Value, json};
use synapse_core::CaptureFrameCursor;
use synapse_test_utils::json_schema::SchemaValidator;

use super::{
    super::schema_sanitize::{sanitize_tools, tagged_object_input_schema},
    LiveCaptureParams, SynapseService,
};

fn published_livecapture_input_schema() -> anyhow::Result<Value> {
    let raw_schema = tagged_object_input_schema::<LiveCaptureParams>();
    let tool = Tool::new("livecapture", "livecapture test tool", raw_schema);
    let sanitized = sanitize_tools(vec![tool]);
    let first = sanitized
        .first()
        .context("sanitized tools list must contain livecapture")?;
    Ok(Value::Object((*first.input_schema).clone()))
}

fn expected_cursor() -> CaptureFrameCursor {
    CaptureFrameCursor {
        capture_id: "cap-109".to_owned(),
        generation: 3,
        frame_seq: 9,
    }
}

fn parse_next_cursor(args: &Value) -> anyhow::Result<CaptureFrameCursor> {
    match serde_json::from_value::<LiveCaptureParams>(args.clone())
        .context("deserialize LiveCaptureParams")?
    {
        LiveCaptureParams::Next { after_frame, .. } => Ok(after_frame),
        other => anyhow::bail!("expected LiveCaptureParams::Next, got {other:?}"),
    }
}

#[test]
fn must_pass_published_schema_and_deserializer_accept_echoed_frame_and_string_cursor()
-> anyhow::Result<()> {
    let schema = published_livecapture_input_schema()?;
    let validator = SchemaValidator::new(&schema);
    let cases = [
        json!({
            "operation": "next",
            "window_hwnd": 0x1090,
            "after_frame": {
                "capture_id": "cap-109",
                "generation": 3,
                "frame_seq": 9
            }
        }),
        json!({
            "operation": "next",
            "window_hwnd": 0x1090,
            "after_frame": {
                "capture_id": "cap-109",
                "generation": 3,
                "frame_seq": 9,
                "dirty_region": {"x": 0, "y": 0, "w": 320, "h": 240},
                "frame_reused": false,
                "sample_age_ms": 4
            }
        }),
        json!({
            "operation": "next",
            "window_hwnd": 0x1090,
            "after_frame": "{\"capture_id\":\"cap-109\",\"generation\":3,\"frame_seq\":9}"
        }),
        json!({
            "operation": "next",
            "window_hwnd": 0x1090,
            "after_frame": "{\"capture_id\":\"cap-109\",\"generation\":3,\"frame_seq\":9,\"dirty_region\":null,\"frame_reused\":true,\"sample_age_ms\":11}"
        }),
    ];
    for (index, args) in cases.iter().enumerate() {
        validator.validate(args).map_err(|error| {
            anyhow::anyhow!(
                "case {index} rejected by published livecapture inputSchema: {error} (args={args})"
            )
        })?;
        ensure!(
            parse_next_cursor(args)? == expected_cursor(),
            "case {index} cursor mismatch"
        );
    }
    Ok(())
}

#[test]
fn must_fail_livecapture_next_rejects_non_object_json_strings_and_missing_fields()
-> anyhow::Result<()> {
    let invalid_cases = [
        json!({"operation": "next", "window_hwnd": 1, "after_frame": "[1, 2, 3]"}),
        json!({"operation": "next", "window_hwnd": 1, "after_frame": "42"}),
        json!({"operation": "next", "window_hwnd": 1, "after_frame": "true"}),
        json!({"operation": "next", "window_hwnd": 1, "after_frame": "null"}),
        json!({"operation": "next", "window_hwnd": 1, "after_frame": "{}"}),
        json!({"operation": "next", "window_hwnd": 1, "after_frame": "{\"capture_id\":\"c\",\"generation\":1}"}),
        json!({"operation": "next", "window_hwnd": 1, "after_frame": {"capture_id": "c", "frame_seq": 2}}),
    ];
    for (index, args) in invalid_cases.iter().enumerate() {
        let result = serde_json::from_value::<LiveCaptureParams>(args.clone());
        ensure!(
            result.is_err(),
            "invalid case {index} unexpectedly deserialized: {result:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn must_fail_normalized_cursors_dispatch_into_livecapture_frame_pipeline()
-> anyhow::Result<()> {
    let dir = tempfile::TempDir::new().context("tempdir")?;
    let service = SynapseService::try_new_isolated_for_tests(dir.path())?;
    let echoed = parse_next_cursor(&json!({
        "operation": "next",
        "window_hwnd": 0x1090,
        "after_frame": {
            "capture_id": "cap-109",
            "generation": 3,
            "frame_seq": 9,
            "dirty_region": null,
            "frame_reused": false,
            "sample_age_ms": 0
        }
    }))?;
    let stringified = parse_next_cursor(&json!({
        "operation": "next",
        "window_hwnd": 0x1090,
        "after_frame": "{\"capture_id\":\"cap-109\",\"generation\":3,\"frame_seq\":9,\"frame_reused\":true}"
    }))?;
    ensure!(
        echoed == stringified,
        "both forms must normalize to the same cursor"
    );
    let Err(error) = service
        .next_live_frame(0x1090, Some(&echoed), Duration::from_millis(10))
        .await
    else {
        anyhow::bail!("next_live_frame without active capture must fail");
    };
    ensure!(
        error.reason == "capture_inactive",
        "expected capture_inactive from handler pipeline, got {}",
        error.reason
    );
    Ok(())
}
