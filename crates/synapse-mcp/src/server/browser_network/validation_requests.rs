//! Validation and filtering for the request, requests and websockets reads.

use super::normalized::{
    NormalizedBrowserNetworkRequestParams, NormalizedBrowserNetworkRequestsParams,
    NormalizedBrowserNetworkWebSocketsParams,
};
use super::types_read::{
    BrowserNetworkRequestParams, BrowserNetworkRequestsParams, BrowserNetworkWebSocketsParams,
};
use super::{
    DEFAULT_NETWORK_REQUEST_LIMIT, MAX_NETWORK_FILTER_CHARS, MAX_NETWORK_REQUEST_ID_CHARS,
    MAX_NETWORK_REQUEST_LIMIT, MAX_NETWORK_RESOURCE_TYPE_CHARS, REQUEST_TOOL, REQUESTS_TOOL,
    WEBSOCKETS_TOOL,
};
use crate::m1::mcp_error;
use crate::server::ErrorData;
use crate::server::m1_tools::validate_cdp_target_id;
use synapse_core::error_codes;

pub(super) fn validate_browser_network_requests_params(
    params: &BrowserNetworkRequestsParams,
) -> Result<NormalizedBrowserNetworkRequestsParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    let limit = params.limit.unwrap_or(DEFAULT_NETWORK_REQUEST_LIMIT);
    if !(1..=MAX_NETWORK_REQUEST_LIMIT).contains(&limit) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{REQUESTS_TOOL} limit must be 1..={MAX_NETWORK_REQUEST_LIMIT}"),
        ));
    }
    if params.url_contains.is_some() && params.url_regex.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{REQUESTS_TOOL} accepts url_contains or url_regex, not both"),
        ));
    }
    let url_contains = validate_text_filter("url_contains", params.url_contains.as_deref())?;
    let url_regex_pattern = validate_text_filter("url_regex", params.url_regex.as_deref())?;
    let url_regex = url_regex_pattern
        .as_deref()
        .map(|pattern| {
            regex::Regex::new(pattern).map_err(|error| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("{REQUESTS_TOOL} url_regex is invalid: {error}"),
                )
            })
        })
        .transpose()?;
    let resource_type =
        validate_resource_type_for_tool(REQUESTS_TOOL, params.resource_type.as_deref())?;
    validate_status_bound_for_tool(REQUESTS_TOOL, "status_min", params.status_min)?;
    validate_status_bound_for_tool(REQUESTS_TOOL, "status_max", params.status_max)?;
    if let (Some(min), Some(max)) = (params.status_min, params.status_max)
        && min > max
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{REQUESTS_TOOL} status_min must be <= status_max"),
        ));
    }
    Ok(NormalizedBrowserNetworkRequestsParams {
        since_seq: params.since_seq,
        limit,
        url_contains,
        url_regex_pattern,
        url_regex,
        resource_type,
        status_min: params.status_min,
        status_max: params.status_max,
    })
}

pub(super) fn validate_browser_network_websockets_params(
    params: &BrowserNetworkWebSocketsParams,
) -> Result<NormalizedBrowserNetworkWebSocketsParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    let limit = params.limit.unwrap_or(DEFAULT_NETWORK_REQUEST_LIMIT);
    if !(1..=MAX_NETWORK_REQUEST_LIMIT).contains(&limit) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{WEBSOCKETS_TOOL} limit must be 1..={MAX_NETWORK_REQUEST_LIMIT}"),
        ));
    }
    let request_id = params
        .request_id
        .as_deref()
        .map(|request_id| validate_request_id_for_tool(WEBSOCKETS_TOOL, request_id))
        .transpose()?;
    let url_contains = validate_text_filter_for_tool(
        WEBSOCKETS_TOOL,
        "url_contains",
        params.url_contains.as_deref(),
    )?;
    Ok(NormalizedBrowserNetworkWebSocketsParams {
        since_seq: params.since_seq,
        limit,
        request_id,
        url_contains,
    })
}

pub(super) fn validate_browser_network_request_params(
    params: &BrowserNetworkRequestParams,
) -> Result<NormalizedBrowserNetworkRequestParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    let request_id = validate_request_id(&params.request_id)?;
    Ok(NormalizedBrowserNetworkRequestParams {
        request_id,
        include_body: params.include_body,
        include_post_data: params.include_post_data,
    })
}

fn validate_request_id(request_id: &str) -> Result<String, ErrorData> {
    validate_request_id_for_tool(REQUEST_TOOL, request_id)
}

fn validate_request_id_for_tool(tool: &str, request_id: &str) -> Result<String, ErrorData> {
    if request_id.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} request_id must not be empty"),
        ));
    }
    if request_id.trim() != request_id {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} request_id must not contain leading or trailing whitespace"),
        ));
    }
    if request_id.contains('\0') || request_id.chars().any(char::is_control) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} request_id must not contain control characters"),
        ));
    }
    if request_id.chars().count() > MAX_NETWORK_REQUEST_ID_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{tool} request_id must be at most {MAX_NETWORK_REQUEST_ID_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(request_id.to_owned())
}

fn validate_text_filter(field: &str, value: Option<&str>) -> Result<Option<String>, ErrorData> {
    validate_text_filter_for_tool(REQUESTS_TOOL, field, value)
}

pub(super) fn validate_text_filter_for_tool(
    tool: &str,
    field: &str,
    value: Option<&str>,
) -> Result<Option<String>, ErrorData> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} {field} must not be empty"),
        ));
    }
    if value.contains('\0') {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} {field} must not contain NUL"),
        ));
    }
    if value.chars().count() > MAX_NETWORK_FILTER_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{tool} {field} must be at most {MAX_NETWORK_FILTER_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(Some(value.to_owned()))
}

pub(super) fn validate_resource_type_for_tool(
    tool_name: &str,
    value: Option<&str>,
) -> Result<Option<String>, ErrorData> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool_name} resource_type must not be empty"),
        ));
    }
    if value.trim() != value {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool_name} resource_type must not contain leading or trailing whitespace"),
        ));
    }
    if value.contains('\0') || value.chars().any(char::is_control) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool_name} resource_type must not contain control characters"),
        ));
    }
    if value.chars().count() > MAX_NETWORK_RESOURCE_TYPE_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{tool_name} resource_type must be at most {MAX_NETWORK_RESOURCE_TYPE_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(Some(value.to_owned()))
}

pub(super) fn validate_status_bound_for_tool(
    tool: &str,
    field: &str,
    value: Option<i64>,
) -> Result<(), ErrorData> {
    if let Some(value) = value
        && !(0..=999).contains(&value)
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{tool} {field} must be 0..=999"),
        ));
    }
    Ok(())
}

pub(super) fn filter_network_entries(
    entries: impl Iterator<Item = synapse_a11y::CdpNetworkEntry>,
    filters: &NormalizedBrowserNetworkRequestsParams,
) -> Vec<synapse_a11y::CdpNetworkEntry> {
    entries
        .filter(|entry| network_entry_matches(entry, filters))
        .collect()
}

fn network_entry_matches(
    entry: &synapse_a11y::CdpNetworkEntry,
    filters: &NormalizedBrowserNetworkRequestsParams,
) -> bool {
    if let Some(resource_type) = filters.resource_type.as_deref()
        && !entry
            .resource_type
            .as_deref()
            .is_some_and(|entry_type| entry_type.eq_ignore_ascii_case(resource_type))
    {
        return false;
    }
    let status = entry.response.as_ref().map(|response| response.status);
    if let Some(min) = filters.status_min
        && !status.is_some_and(|status| status >= min)
    {
        return false;
    }
    if let Some(max) = filters.status_max
        && !status.is_some_and(|status| status <= max)
    {
        return false;
    }
    if let Some(needle) = filters.url_contains.as_deref()
        && !entry
            .url
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(&needle.to_lowercase())
    {
        return false;
    }
    if let Some(regex) = filters.url_regex.as_ref()
        && !entry.url.as_deref().is_some_and(|url| regex.is_match(url))
    {
        return false;
    }
    true
}
