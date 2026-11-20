//! #89: facade descriptions and profile policy text must state the public tool ceiling
//! from the registry authority (`PUBLIC_TOOL_LIMIT`), never a stale literal.

use super::{PUBLIC_TOOL_LIMIT, ToolProfileKind, foreground_capability_policy};
use crate::server::{SynapseService, schema_sanitize::sanitize_tools};

/// Every number written as `<=N` in `text`.
fn stated_ceilings(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    for (offset, _) in text.match_indices("<=") {
        let digits: String = text[offset + 2..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(value) = digits.parse::<usize>() {
            found.push(value);
        }
    }
    found
}

#[test]
fn must_pass_listed_tool_descriptions_state_the_current_ceiling() {
    let tools = sanitize_tools(SynapseService::tool_router().list_all());
    let mut stating = 0_usize;
    for tool in &tools {
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            !description.contains("{PUBLIC_TOOL_LIMIT}"),
            "{} leaves the ceiling placeholder unexpanded",
            tool.name
        );
        for ceiling in stated_ceilings(description) {
            assert_eq!(
                ceiling, PUBLIC_TOOL_LIMIT,
                "{} states a stale tool ceiling <={ceiling}",
                tool.name
            );
            stating += 1;
        }
    }
    assert!(
        stating >= 10,
        "expected many facade descriptions to state the ceiling, found {stating}"
    );
}

#[test]
fn must_fail_profile_foreground_policy_text_has_no_stale_ceiling() {
    for profile in [
        ToolProfileKind::NormalAgent,
        ToolProfileKind::BrowserControl,
        ToolProfileKind::BrowserDebugger,
        ToolProfileKind::BreakGlass,
        ToolProfileKind::FullCapability,
    ] {
        let policy = foreground_capability_policy(profile);
        for text in [policy.preferred_path, policy.real_os_foreground_path] {
            for ceiling in stated_ceilings(text) {
                assert_eq!(ceiling, PUBLIC_TOOL_LIMIT, "{profile:?}: {text}");
            }
        }
    }
}
