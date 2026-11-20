#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(feature = "livefeed")]

use std::collections::BTreeSet;

use serde_json::{Value, json};
use synapse_test_utils::json_schema::{SchemaValidator, strict_client_schema_issues};

use crate::server::{
    SynapseService,
    schema_sanitize::sanitize_tools,
    tool_profiles::{PUBLIC_TOOL_LIMIT, PUBLIC_TOOL_NAMES},
};

#[test]
fn must_pass_livefeed_facade_schema_and_tool_count_44() {
    let tools = sanitize_tools(SynapseService::tool_router().list_all());
    let public: Vec<_> = tools
        .iter()
        .filter(|tool| PUBLIC_TOOL_NAMES.contains(&tool.name.as_ref()))
        .collect();
    let names: BTreeSet<_> = public.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(PUBLIC_TOOL_LIMIT, 44);
    assert_eq!(PUBLIC_TOOL_NAMES.len(), 44);
    assert_eq!(public.len(), 44, "all 44 public tools must be registered");
    assert_eq!(
        names.len(),
        44,
        "duplicate registration is not another tool"
    );
    assert!(names.contains("livefeed"));
    for tool in &public {
        for schema in std::iter::once(&tool.input_schema).chain(tool.output_schema.iter()) {
            let schema = Value::Object((**schema).clone());
            assert!(
                strict_client_schema_issues(&schema).is_empty(),
                "{} breaks strict tools/list: {schema}",
                tool.name
            );
        }
    }
    let tool = public
        .iter()
        .find(|tool| tool.name == "livefeed")
        .expect("livefeed tool");
    let schema = Value::Object((*tool.input_schema).clone());
    assert_eq!(schema["type"], "object", "client requires object root");
    validate_operations(&schema);
}

fn validate_operations(schema: &Value) {
    let validator = SchemaValidator::new(schema);
    for args in [
        json!({"operation":"start"}),
        json!({"operation":"start", "monitor":0}),
        json!({"operation":"stop"}),
        json!({"operation":"look"}),
        json!({"operation":"status"}),
        json!({"operation":"changes", "since":0}),
        json!({"operation":"changes", "since":23, "wait_ms":250}),
        json!({"operation":"zoom", "x":5, "y":7, "w":20, "h":30}),
    ] {
        validator.validate(&args).unwrap_or_else(|error| {
            panic!("valid livefeed args {args}: {error}");
        });
    }
    for args in [
        json!({}),
        json!({"operation":"unknown"}),
        json!({"operation":"changes_since", "cursor":0}),
        json!({"operation":"frame_at", "ts_ms":1234}),
        json!({"operation":"crop", "x":5, "y":7, "w":20, "h":30}),
        json!({"operation":"changes"}),
        json!({"operation":"changes", "since":-1}),
        json!({"operation":"changes", "since":0, "wait_ms":"soon"}),
        json!({"operation":"start", "monitor":"primary"}),
        json!({"operation":"zoom", "x":5, "y":7, "w":20}),
    ] {
        assert!(
            validator.validate(&args).is_err(),
            "invalid args accepted: {args}"
        );
    }
}
