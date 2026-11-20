//! Validation for `browser_network_overrides` params.

use super::normalized::NormalizedBrowserNetworkOverridesParams;
use super::types_har_overrides::{BrowserNetworkOverridesOperation, BrowserNetworkOverridesParams};
use super::validation_route::validate_route_headers;
use super::{MAX_NETWORK_USER_AGENT_CHARS, OVERRIDES_TOOL};
use crate::m1::mcp_error;
use crate::server::ErrorData;
use crate::server::m1_tools::validate_cdp_target_id;
use synapse_core::error_codes;

pub(super) fn validate_browser_network_overrides_params(
    params: &BrowserNetworkOverridesParams,
) -> Result<NormalizedBrowserNetworkOverridesParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    match params.operation {
        BrowserNetworkOverridesOperation::Set => {
            let headers = validate_route_headers(&params.headers)?;
            let user_agent = validate_network_user_agent(params.user_agent.as_deref())?;
            Ok(NormalizedBrowserNetworkOverridesParams {
                operation: params.operation,
                headers,
                user_agent,
            })
        }
        BrowserNetworkOverridesOperation::Get | BrowserNetworkOverridesOperation::Clear => {
            if !params.headers.is_empty() || params.user_agent.is_some() {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!(
                        "{OVERRIDES_TOOL} headers and user_agent are only valid for operation=set"
                    ),
                ));
            }
            Ok(NormalizedBrowserNetworkOverridesParams {
                operation: params.operation,
                headers: Vec::new(),
                user_agent: None,
            })
        }
    }
}

fn validate_network_user_agent(value: Option<&str>) -> Result<Option<String>, ErrorData> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{OVERRIDES_TOOL} user_agent must not be empty"),
        ));
    }
    if value.trim() != value {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{OVERRIDES_TOOL} user_agent must not contain leading or trailing whitespace"),
        ));
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{OVERRIDES_TOOL} user_agent must not contain line breaks or NUL"),
        ));
    }
    if value.chars().count() > MAX_NETWORK_USER_AGENT_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{OVERRIDES_TOOL} user_agent must be at most {MAX_NETWORK_USER_AGENT_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(Some(value.to_owned()))
}
