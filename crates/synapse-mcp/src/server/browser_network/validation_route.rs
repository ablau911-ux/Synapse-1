//! Validation and normalization for `browser_route` params.

use super::normalized::NormalizedBrowserRouteParams;
use super::route_body::{
    generate_route_id, normalize_route_body, normalize_route_continue_post_data,
};
use super::types_route::{
    BrowserRouteHeader, BrowserRouteMatchKind, BrowserRouteOperation, BrowserRouteParams,
};
use super::validation_requests::validate_resource_type_for_tool;
use super::{
    MAX_ROUTE_HEADER_COUNT, MAX_ROUTE_HEADER_NAME_CHARS, MAX_ROUTE_HEADER_VALUE_CHARS,
    MAX_ROUTE_ID_CHARS, MAX_ROUTE_RESPONSE_PHRASE_CHARS, MAX_ROUTE_URL_CHARS, ROUTE_TOOL,
};
use crate::m1::mcp_error;
use crate::server::ErrorData;
use crate::server::m1_tools::validate_cdp_target_id;
use synapse_core::error_codes;

pub(super) fn validate_browser_route_params(
    params: &BrowserRouteParams,
) -> Result<NormalizedBrowserRouteParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    let route_id = match params.operation {
        BrowserRouteOperation::AddFulfill
        | BrowserRouteOperation::AddAbort
        | BrowserRouteOperation::AddContinue => params
            .route_id
            .as_deref()
            .map(validate_route_id)
            .transpose()?
            .unwrap_or_else(generate_route_id),
        BrowserRouteOperation::Remove => {
            validate_route_id(params.route_id.as_deref().ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("{ROUTE_TOOL} route_id is required for remove"),
                )
            })?)?
        }
        BrowserRouteOperation::Clear | BrowserRouteOperation::List => params
            .route_id
            .as_deref()
            .map(validate_route_id)
            .transpose()?
            .unwrap_or_default(),
    };
    let route_id = if route_id.is_empty() {
        None
    } else {
        Some(route_id)
    };
    let route = match params.operation {
        BrowserRouteOperation::AddFulfill => Some(normalize_route_fulfill(
            params,
            route_id.clone().ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("{ROUTE_TOOL} add_fulfill missing generated route_id"),
                )
            })?,
        )?),
        BrowserRouteOperation::AddAbort => Some(normalize_route_abort(
            params,
            route_id.clone().ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("{ROUTE_TOOL} add_abort missing generated route_id"),
                )
            })?,
        )?),
        BrowserRouteOperation::AddContinue => Some(normalize_route_continue(
            params,
            route_id.clone().ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("{ROUTE_TOOL} add_continue missing generated route_id"),
                )
            })?,
        )?),
        BrowserRouteOperation::Remove
        | BrowserRouteOperation::Clear
        | BrowserRouteOperation::List => None,
    };
    Ok(NormalizedBrowserRouteParams {
        operation: params.operation,
        route_id,
        route,
    })
}

fn normalize_route_fulfill(
    params: &BrowserRouteParams,
    route_id: String,
) -> Result<synapse_a11y::CdpFetchRouteRule, ErrorData> {
    let (url, resource_type) = normalize_route_match(params, "add_fulfill")?;
    let status = params.status.unwrap_or(200);
    if !(100..=599).contains(&status) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} status must be 100..=599"),
        ));
    }
    if params.error_reason.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} error_reason is only valid for add_abort"),
        ));
    }
    let response_phrase = validate_route_response_phrase(params.response_phrase.as_deref())?;
    let headers = validate_route_headers(&params.headers)?;
    let body_base64 = normalize_route_body(params)?;
    Ok(synapse_a11y::CdpFetchRouteRule {
        id: route_id,
        url,
        match_kind: match params.match_kind {
            BrowserRouteMatchKind::Glob => synapse_a11y::CdpFetchRouteMatchKind::Glob,
            BrowserRouteMatchKind::Regex => synapse_a11y::CdpFetchRouteMatchKind::Regex,
        },
        method: None,
        resource_type,
        action: synapse_a11y::CdpFetchRouteAction::Fulfill(synapse_a11y::CdpFetchRouteFulfill {
            status,
            response_phrase,
            headers,
            body_base64,
        }),
    })
}

fn normalize_route_abort(
    params: &BrowserRouteParams,
    route_id: String,
) -> Result<synapse_a11y::CdpFetchRouteRule, ErrorData> {
    let (url, resource_type) = normalize_route_match(params, "add_abort")?;
    if params.status.is_some()
        || params.response_phrase.is_some()
        || !params.headers.is_empty()
        || params.body.is_some()
        || params.body_base64.is_some()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{ROUTE_TOOL} status, response_phrase, headers, body, and body_base64 are only valid for add_fulfill"
            ),
        ));
    }
    let error_reason = params.error_reason.unwrap_or_default();
    Ok(synapse_a11y::CdpFetchRouteRule {
        id: route_id,
        url,
        match_kind: match params.match_kind {
            BrowserRouteMatchKind::Glob => synapse_a11y::CdpFetchRouteMatchKind::Glob,
            BrowserRouteMatchKind::Regex => synapse_a11y::CdpFetchRouteMatchKind::Regex,
        },
        method: None,
        resource_type,
        action: synapse_a11y::CdpFetchRouteAction::Abort(synapse_a11y::CdpFetchRouteAbort {
            error_reason: error_reason.as_cdp_str().to_owned(),
        }),
    })
}

fn normalize_route_continue(
    params: &BrowserRouteParams,
    route_id: String,
) -> Result<synapse_a11y::CdpFetchRouteRule, ErrorData> {
    let (url, resource_type) = normalize_route_match(params, "add_continue")?;
    if params.status.is_some()
        || params.response_phrase.is_some()
        || !params.headers.is_empty()
        || params.body.is_some()
        || params.body_base64.is_some()
        || params.error_reason.is_some()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{ROUTE_TOOL} status, response_phrase, headers, body, body_base64, and error_reason are not valid for add_continue"
            ),
        ));
    }
    if params.continue_url.is_none()
        && params.continue_method.is_none()
        && params.continue_headers.is_empty()
        && params.continue_post_data.is_none()
        && params.continue_post_data_base64.is_none()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} add_continue requires at least one override"),
        ));
    }
    let continue_url = params
        .continue_url
        .as_deref()
        .map(validate_route_url)
        .transpose()?;
    let continue_method = params
        .continue_method
        .as_deref()
        .map(validate_route_method)
        .transpose()?;
    let headers = validate_route_headers(&params.continue_headers)?;
    let post_data_base64 = normalize_route_continue_post_data(params)?;
    Ok(synapse_a11y::CdpFetchRouteRule {
        id: route_id,
        url,
        match_kind: match params.match_kind {
            BrowserRouteMatchKind::Glob => synapse_a11y::CdpFetchRouteMatchKind::Glob,
            BrowserRouteMatchKind::Regex => synapse_a11y::CdpFetchRouteMatchKind::Regex,
        },
        method: None,
        resource_type,
        action: synapse_a11y::CdpFetchRouteAction::Continue(synapse_a11y::CdpFetchRouteContinue {
            url: continue_url,
            method: continue_method,
            headers,
            post_data_base64,
        }),
    })
}

fn normalize_route_match(
    params: &BrowserRouteParams,
    operation: &str,
) -> Result<(String, Option<String>), ErrorData> {
    let url = validate_route_url(params.url.as_deref().ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} url is required for {operation}"),
        )
    })?)?;
    if matches!(params.match_kind, BrowserRouteMatchKind::Regex) {
        regex::Regex::new(&url).map_err(|error| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{ROUTE_TOOL} url regex is invalid: {error}"),
            )
        })?;
    }
    let resource_type =
        validate_resource_type_for_tool(ROUTE_TOOL, params.resource_type.as_deref())?;
    Ok((url, resource_type))
}

fn validate_route_id(route_id: &str) -> Result<String, ErrorData> {
    if route_id.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} route_id must not be empty"),
        ));
    }
    if route_id.trim() != route_id {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} route_id must not contain leading or trailing whitespace"),
        ));
    }
    if route_id.contains('\0') || route_id.chars().any(char::is_control) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} route_id must not contain control characters"),
        ));
    }
    if route_id.chars().any(char::is_whitespace) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} route_id must not contain whitespace"),
        ));
    }
    if route_id.chars().count() > MAX_ROUTE_ID_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{ROUTE_TOOL} route_id must be at most {MAX_ROUTE_ID_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(route_id.to_owned())
}

fn validate_route_url(url: &str) -> Result<String, ErrorData> {
    if url.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} url must not be empty"),
        ));
    }
    if url.trim() != url {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} url must not contain leading or trailing whitespace"),
        ));
    }
    if url.contains('\0') || url.chars().any(char::is_control) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} url must not contain control characters"),
        ));
    }
    if url.chars().count() > MAX_ROUTE_URL_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} url must be at most {MAX_ROUTE_URL_CHARS} Unicode scalar values"),
        ));
    }
    Ok(url.to_owned())
}

fn validate_route_method(method: &str) -> Result<String, ErrorData> {
    if method.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} continue_method must not be empty"),
        ));
    }
    if method.trim() != method {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} continue_method must not contain leading or trailing whitespace"),
        ));
    }
    if method.bytes().any(|byte| {
        byte <= 0x20
            || byte >= 0x7f
            || matches!(
                byte,
                b'(' | b')'
                    | b'<'
                    | b'>'
                    | b'@'
                    | b','
                    | b';'
                    | b':'
                    | b'\\'
                    | b'"'
                    | b'/'
                    | b'['
                    | b']'
                    | b'?'
                    | b'='
                    | b'{'
                    | b'}'
            )
    }) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} continue_method {method:?} contains an invalid byte"),
        ));
    }
    Ok(method.to_owned())
}

fn validate_route_response_phrase(value: Option<&str>) -> Result<Option<String>, ErrorData> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.contains(['\r', '\n', '\0']) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} response_phrase must not contain line breaks or NUL"),
        ));
    }
    if value.chars().count() > MAX_ROUTE_RESPONSE_PHRASE_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{ROUTE_TOOL} response_phrase must be at most {MAX_ROUTE_RESPONSE_PHRASE_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(Some(value.to_owned()))
}

pub(super) fn validate_route_headers(
    headers: &[BrowserRouteHeader],
) -> Result<Vec<(String, String)>, ErrorData> {
    if headers.len() > MAX_ROUTE_HEADER_COUNT {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} headers must contain at most {MAX_ROUTE_HEADER_COUNT} entries"),
        ));
    }
    headers
        .iter()
        .map(|header| {
            validate_route_header_name(&header.name)?;
            validate_route_header_value(&header.value)?;
            Ok((header.name.clone(), header.value.clone()))
        })
        .collect()
}

fn validate_route_header_name(value: &str) -> Result<(), ErrorData> {
    if value.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} header name must not be empty"),
        ));
    }
    if value.trim() != value {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} header name must not contain leading or trailing whitespace"),
        ));
    }
    if value.chars().count() > MAX_ROUTE_HEADER_NAME_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{ROUTE_TOOL} header name must be at most {MAX_ROUTE_HEADER_NAME_CHARS} Unicode scalar values"
            ),
        ));
    }
    if value.bytes().any(|byte| {
        byte <= 0x20
            || byte >= 0x7f
            || matches!(
                byte,
                b'(' | b')'
                    | b'<'
                    | b'>'
                    | b'@'
                    | b','
                    | b';'
                    | b':'
                    | b'\\'
                    | b'"'
                    | b'/'
                    | b'['
                    | b']'
                    | b'?'
                    | b'='
                    | b'{'
                    | b'}'
            )
    }) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} header name {value:?} contains an invalid byte"),
        ));
    }
    Ok(())
}

fn validate_route_header_value(value: &str) -> Result<(), ErrorData> {
    if value.contains(['\r', '\n', '\0']) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{ROUTE_TOOL} header value must not contain line breaks or NUL"),
        ));
    }
    if value.chars().count() > MAX_ROUTE_HEADER_VALUE_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{ROUTE_TOOL} header value must be at most {MAX_ROUTE_HEADER_VALUE_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(())
}
