//! #221: exercise the classifier used by the real handler, including audit data.
use super::classify_tool_success;
use rmcp::model::CallToolResult;
use serde_json::json;

#[test]
fn must_fail_active_facade_failure_sets_mcp_error_without_rewriting_payload() {
    for (tool, operation) in [
        ("browser_form", "fill"),
        ("model", "probe"),
        ("storage", "status"),
        ("agent", "stop"),
        ("routine", "automate"),
        ("reality", "audit"),
    ] {
        let mut payload = json!({"operation":operation});
        payload[operation] = json!({"ok":false,"error":{"code":"VARIANT_REFUSED"}});
        let mut result = CallToolResult::structured(payload.clone());
        let (failure, _) = classify_tool_success(tool, &mut result);
        assert_eq!(
            failure.expect("active failure").code.as_deref(),
            Some("VARIANT_REFUSED")
        );
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, Some(payload));
    }
}

#[test]
fn must_fail_bridge_ack_and_direct_browser_aggregates_mark_mcp_error() {
    for (tool, payload) in [
        (
            "browser_debugger",
            json!({"operation":"reload_bridge","command_ack":{"ok":false}}),
        ),
        ("browser_batch", json!({"ok":false,"results":[]})),
        ("browser_fill_form", json!({"ok":false,"fields":[]})),
        ("browser_set_value", json!({"ok":false})),
    ] {
        let mut result = CallToolResult::structured(payload.clone());
        assert!(classify_tool_success(tool, &mut result).0.is_some());
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, Some(payload));
    }
}

#[test]
fn must_pass_page_values_history_and_stored_documents_do_not_become_current_failures() {
    for (tool, payload) in [
        (
            "browser",
            json!({"operation":"evaluate","evaluate":{"ok":true,"value":{"ok":false}}}),
        ),
        (
            "agent",
            json!({"operation":"list","list":{"agents":[{"ok":false}]}}),
        ),
        (
            "routine",
            json!({"operation":"list","list":{"documents":[{"ok":false}]}}),
        ),
        (
            "act",
            json!({"operation":"lease_status","lease":{"held":false}}),
        ),
    ] {
        let mut result = CallToolResult::structured(payload.clone());
        let initial_error = result.is_error;
        assert!(classify_tool_success(tool, &mut result).0.is_none());
        assert_eq!(result.is_error, initial_error);
        assert_eq!(result.structured_content, Some(payload));
    }
}
