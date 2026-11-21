use rmcp::model::ErrorCode;
use serde_json::json;
use synapse_core::error_codes;

use crate::server::{ErrorData, tool_profiles::ToolProfileKind};

/// Facade wrapper for delegated failures (shared envelope, see `delegate_errors`).
pub(super) use crate::server::delegate_errors::wrapped_delegate_error as facade_delegate_error;

pub(super) fn missing_spec(tool: &'static str, operation: &'static str) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!("{tool} operation={operation} missing operation payload"),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "tool": tool,
            "operation": operation,
            "source_of_truth": "MCP request parameters",
            "source_id": operation,
            "remediation": "pass the payload object matching operation",
        })),
    )
}

pub(super) fn facade_policy_error(
    tool: &'static str,
    operation: &'static str,
    source_id: &str,
    profile: ToolProfileKind,
    source_of_truth: &'static str,
    remediation: &'static str,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} operation={operation} is not allowed for profile {}",
            profile.as_str()
        ),
        Some(json!({
            "code": error_codes::TOOL_PROFILE_POLICY_DENIED,
            "tool": tool,
            "operation": operation,
            "source_id": source_id,
            "profile": profile.as_str(),
            "source_of_truth": source_of_truth,
            "remediation": remediation,
        })),
    )
}
