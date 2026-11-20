//! Regression tests for #115: a caller sent analyze-only keys with
//! `operation=inspect` and was rejected by its strict schema validator. The
//! published contract must stay strict (inspect = operation + path only) and
//! the tool/variant descriptions must state the inspect-first call order.

use std::num::NonZeroUsize;

use serde_json::Value;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use crate::server::SynapseService;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

fn service(dir: &std::path::Path) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(dir.join("db")),
            Some(dir.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).ok_or_else(|| anyhow::anyhow!("nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

fn published_videoanalyze() -> anyhow::Result<(String, Value)> {
    let dir = TempDir::new()?;
    let tool = service(dir.path())?
        .full_sanitized_tools()
        .into_iter()
        .find(|tool| tool.name == "videoanalyze")
        .ok_or_else(|| anyhow::anyhow!("videoanalyze not published"))?;
    let description = tool.description.map(|d| d.to_string()).unwrap_or_default();
    Ok((description, Value::Object((*tool.input_schema).clone())))
}

fn inspect_branch(schema: &Value) -> Option<&Value> {
    ["oneOf", "anyOf"]
        .iter()
        .filter_map(|key| schema.get(*key).and_then(Value::as_array))
        .flatten()
        .find(|branch| {
            branch.pointer("/properties/operation/const") == Some(&Value::from("inspect"))
                || branch.pointer("/properties/operation/enum/0") == Some(&Value::from("inspect"))
        })
}

#[test]
fn must_pass_inspect_schema_is_operation_and_path_only() -> anyhow::Result<()> {
    let (_, schema) = published_videoanalyze()?;
    let branch = inspect_branch(&schema).ok_or_else(|| anyhow::anyhow!("no inspect branch"))?;
    let mut keys: Vec<&str> = branch["properties"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("no properties"))?
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["operation", "path"]);
    assert_eq!(branch["additionalProperties"], Value::Bool(false));
    let branch_text = branch.to_string();
    assert!(
        branch_text.contains("path only"),
        "inspect variant doc must say it takes path only: {branch_text}"
    );
    Ok(())
}

#[test]
fn must_pass_description_states_inspect_first_call_order() -> anyhow::Result<()> {
    let (description, _) = published_videoanalyze()?;
    for needle in [
        "inspect takes path only",
        "call inspect first",
        "duration",
        "then call analyze or motion",
    ] {
        assert!(
            description.contains(needle),
            "missing {needle:?} in: {description}"
        );
    }
    Ok(())
}

fn decode(params: Value) -> Result<super::VideoAnalyzeParams, serde_json::Error> {
    serde_json::from_value(params)
}

#[test]
fn must_fail_inspect_with_analyze_only_keys_is_rejected() -> anyhow::Result<()> {
    // Baseline: the strict inspect shape itself decodes.
    assert!(matches!(
        decode(serde_json::json!({"operation": "inspect", "path": "clip.mp4"}))?,
        super::VideoAnalyzeParams::Inspect { .. }
    ));
    for (key, value) in [
        ("start_seconds", serde_json::json!(0.0)),
        ("end_seconds", serde_json::json!(5.0)),
        ("frame_count", serde_json::json!(4)),
        ("include_audio", serde_json::json!(false)),
    ] {
        let mut params = serde_json::json!({"operation": "inspect", "path": "clip.mp4"});
        params[key] = value;
        assert!(
            decode(params).is_err(),
            "inspect must reject analyze-only key {key:?}"
        );
    }
    Ok(())
}

#[test]
fn must_fail_inspect_without_path_or_with_unknown_operation_is_rejected() {
    assert!(
        decode(serde_json::json!({"operation": "inspect"})).is_err(),
        "inspect requires path"
    );
    assert!(
        decode(serde_json::json!({"operation": "probe", "path": "clip.mp4"})).is_err(),
        "unknown operations fail closed"
    );
    assert!(
        decode(serde_json::json!({"path": "clip.mp4"})).is_err(),
        "operation is required"
    );
}
