//! `browser_route` body / post-data normalization and route-id generation.

use super::types_route::BrowserRouteParams;
use super::{MAX_ROUTE_BODY_CHARS, ROUTE_TOOL};
use crate::m1::mcp_error;
use crate::server::ErrorData;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_core::error_codes;

pub(super) fn normalize_route_body(
    params: &BrowserRouteParams,
) -> Result<Option<String>, ErrorData> {
    if params.body.is_some() && params.body_base64.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} accepts body or body_base64, not both"),
        ));
    }
    if let Some(body) = params.body.as_deref() {
        if body.chars().count() > MAX_ROUTE_BODY_CHARS {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{ROUTE_TOOL} body must be at most {MAX_ROUTE_BODY_CHARS} Unicode scalar values"
                ),
            ));
        }
        return Ok(Some(BASE64_STANDARD.encode(body.as_bytes())));
    }
    if let Some(body_base64) = params.body_base64.as_deref() {
        if body_base64.contains('\0') || body_base64.chars().any(char::is_control) {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{ROUTE_TOOL} body_base64 must not contain control characters"),
            ));
        }
        if body_base64.chars().count() > MAX_ROUTE_BODY_CHARS {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{ROUTE_TOOL} body_base64 must be at most {MAX_ROUTE_BODY_CHARS} Unicode scalar values"
                ),
            ));
        }
        BASE64_STANDARD.decode(body_base64).map_err(|error| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{ROUTE_TOOL} body_base64 is invalid: {error}"),
            )
        })?;
        return Ok(Some(body_base64.to_owned()));
    }
    Ok(None)
}

pub(super) fn normalize_route_continue_post_data(
    params: &BrowserRouteParams,
) -> Result<Option<String>, ErrorData> {
    if params.continue_post_data.is_some() && params.continue_post_data_base64.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{ROUTE_TOOL} accepts continue_post_data or continue_post_data_base64, not both"
            ),
        ));
    }
    if let Some(post_data) = params.continue_post_data.as_deref() {
        if post_data.chars().count() > MAX_ROUTE_BODY_CHARS {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{ROUTE_TOOL} continue_post_data must be at most {MAX_ROUTE_BODY_CHARS} Unicode scalar values"
                ),
            ));
        }
        return Ok(Some(BASE64_STANDARD.encode(post_data.as_bytes())));
    }
    if let Some(post_data_base64) = params.continue_post_data_base64.as_deref() {
        if post_data_base64.contains('\0') || post_data_base64.chars().any(char::is_control) {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{ROUTE_TOOL} continue_post_data_base64 must not contain control characters"
                ),
            ));
        }
        if post_data_base64.chars().count() > MAX_ROUTE_BODY_CHARS {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{ROUTE_TOOL} continue_post_data_base64 must be at most {MAX_ROUTE_BODY_CHARS} Unicode scalar values"
                ),
            ));
        }
        BASE64_STANDARD.decode(post_data_base64).map_err(|error| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{ROUTE_TOOL} continue_post_data_base64 is invalid: {error}"),
            )
        })?;
        return Ok(Some(post_data_base64.to_owned()));
    }
    Ok(None)
}

pub(super) fn generate_route_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("route-{millis}-{}", std::process::id())
}
