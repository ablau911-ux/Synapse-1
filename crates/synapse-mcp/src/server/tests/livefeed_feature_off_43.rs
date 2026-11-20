#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(not(feature = "livefeed"))]

use std::collections::BTreeSet;

use crate::server::{
    SynapseService,
    schema_sanitize::sanitize_tools,
    tool_profiles::{PUBLIC_TOOL_LIMIT, PUBLIC_TOOL_NAMES},
};

#[test]
fn must_pass_livefeed_feature_off_keeps_tool_count_43() {
    let tools = sanitize_tools(SynapseService::tool_router().list_all());
    let public: Vec<_> = tools
        .iter()
        .filter(|tool| PUBLIC_TOOL_NAMES.contains(&tool.name.as_ref()))
        .collect();
    let names: BTreeSet<_> = public.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(PUBLIC_TOOL_LIMIT, 43);
    assert_eq!(PUBLIC_TOOL_NAMES.len(), 43);
    assert_eq!(public.len(), 43);
    assert_eq!(names.len(), 43);
    assert!(!PUBLIC_TOOL_NAMES.contains(&"livefeed"));
    assert!(
        !tools.iter().any(|tool| tool.name == "livefeed"),
        "disabled feature must register no livefeed tool"
    );
}
