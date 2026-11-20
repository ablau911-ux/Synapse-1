//! Validation for `browser_network_har` params.

use super::normalized::{
    NormalizedBrowserNetworkHarParams, NormalizedBrowserNetworkRequestsParams,
};
use super::types_har_overrides::{BrowserNetworkHarOperation, BrowserNetworkHarParams};
use super::validation_har_path::validate_har_path;
use super::validation_requests::{
    validate_resource_type_for_tool, validate_status_bound_for_tool, validate_text_filter_for_tool,
};
use super::{
    DEFAULT_NETWORK_REQUEST_LIMIT, HAR_TOOL, MAX_HAR_REPLAY_ENTRIES, MAX_NETWORK_REQUEST_LIMIT,
};
use crate::m1::mcp_error;
use crate::server::ErrorData;
use crate::server::m1_tools::validate_cdp_target_id;
use synapse_core::error_codes;

pub(super) fn validate_browser_network_har_params(
    params: &BrowserNetworkHarParams,
) -> Result<NormalizedBrowserNetworkHarParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    let path = match params.operation {
        BrowserNetworkHarOperation::Record | BrowserNetworkHarOperation::Replay => Some(
            validate_har_path(params.path.as_deref().ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("{HAR_TOOL} path is required for {:?}", params.operation),
                )
            })?)?,
        ),
        BrowserNetworkHarOperation::ClearReplay => {
            if params.path.is_some() {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("{HAR_TOOL} path is only valid for record and replay"),
                ));
            }
            None
        }
    };
    if params.operation != BrowserNetworkHarOperation::Record {
        if params.since_seq.is_some()
            || params.url_contains.is_some()
            || params.url_regex.is_some()
            || params.resource_type.is_some()
            || params.status_min.is_some()
            || params.status_max.is_some()
            || params.include_bodies.is_some()
            || params.overwrite.is_some()
        {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{HAR_TOOL} capture filters, include_bodies and overwrite are only valid for record"
                ),
            ));
        }
    }
    if params.operation != BrowserNetworkHarOperation::Replay {
        if params.missing_policy.is_some() || params.clear_existing_replay.is_some() {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{HAR_TOOL} missing_policy and clear_existing_replay are only valid for replay"
                ),
            ));
        }
        if params.operation == BrowserNetworkHarOperation::ClearReplay && params.limit.is_some() {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{HAR_TOOL} limit is only valid for record and replay"),
            ));
        }
    }
    let limit = match params.operation {
        BrowserNetworkHarOperation::Record => params.limit.unwrap_or(DEFAULT_NETWORK_REQUEST_LIMIT),
        BrowserNetworkHarOperation::Replay => params.limit.unwrap_or(MAX_HAR_REPLAY_ENTRIES),
        BrowserNetworkHarOperation::ClearReplay => 1,
    };
    let max_limit = match params.operation {
        BrowserNetworkHarOperation::Record => MAX_NETWORK_REQUEST_LIMIT,
        BrowserNetworkHarOperation::Replay => MAX_HAR_REPLAY_ENTRIES,
        BrowserNetworkHarOperation::ClearReplay => 1,
    };
    if !(1..=max_limit).contains(&limit) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{HAR_TOOL} limit must be 1..={max_limit}"),
        ));
    }
    if params.url_contains.is_some() && params.url_regex.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{HAR_TOOL} accepts url_contains or url_regex, not both"),
        ));
    }
    let url_contains =
        validate_text_filter_for_tool(HAR_TOOL, "url_contains", params.url_contains.as_deref())?;
    let url_regex_pattern =
        validate_text_filter_for_tool(HAR_TOOL, "url_regex", params.url_regex.as_deref())?;
    let url_regex = url_regex_pattern
        .as_deref()
        .map(|pattern| {
            regex::Regex::new(pattern).map_err(|error| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("{HAR_TOOL} url_regex is invalid: {error}"),
                )
            })
        })
        .transpose()?;
    let resource_type = validate_resource_type_for_tool(HAR_TOOL, params.resource_type.as_deref())?;
    validate_status_bound_for_tool(HAR_TOOL, "status_min", params.status_min)?;
    validate_status_bound_for_tool(HAR_TOOL, "status_max", params.status_max)?;
    if let (Some(min), Some(max)) = (params.status_min, params.status_max)
        && min > max
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{HAR_TOOL} status_min must be <= status_max"),
        ));
    }
    let (path, path_display) = match path {
        Some((path, display)) => (Some(path), Some(display)),
        None => (None, None),
    };
    Ok(NormalizedBrowserNetworkHarParams {
        operation: params.operation,
        path,
        path_display,
        filters: NormalizedBrowserNetworkRequestsParams {
            since_seq: params.since_seq,
            limit,
            url_contains,
            url_regex_pattern,
            url_regex,
            resource_type,
            status_min: params.status_min,
            status_max: params.status_max,
        },
        include_bodies: params.include_bodies.unwrap_or(false),
        overwrite: params.overwrite.unwrap_or(false),
        missing_policy: params.missing_policy.unwrap_or_default(),
        clear_existing_replay: params.clear_existing_replay.unwrap_or(true),
    })
}
