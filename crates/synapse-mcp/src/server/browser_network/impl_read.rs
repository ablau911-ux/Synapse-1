//! Raw CDP implementations for the request, requests and websockets reads.

use super::normalized::{
    NormalizedBrowserNetworkRequestParams, NormalizedBrowserNetworkRequestsParams,
    NormalizedBrowserNetworkWebSocketsParams,
};
use super::types_read::{
    BrowserNetworkRequestResponse, BrowserNetworkRequestsResponse, BrowserNetworkWebSocketsResponse,
};
use super::validation_requests::filter_network_entries;
use super::wire::{
    browser_network_entry_to_wire, browser_network_post_data_to_wire,
    browser_network_request_detail_to_wire, browser_network_response_body_to_wire,
    browser_network_websocket_entry_to_wire, require_response_body_available,
};
use super::{REQUEST_TOOL, REQUESTS_TOOL, WEBSOCKETS_TOOL};
use crate::m1::mcp_error;
use crate::server::m1_tools::browser_raw_cdp_required_error;
use crate::server::{ErrorData, SynapseService};
use synapse_core::error_codes;

impl SynapseService {
    #[cfg(windows)]
    pub(super) async fn browser_network_requests_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        filters: &NormalizedBrowserNetworkRequestsParams,
    ) -> Result<BrowserNetworkRequestsResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(REQUESTS_TOOL, window_hwnd));
        };
        let capture = synapse_a11y::network_capture_ensure(
            &endpoint,
            cdp_target_id,
            synapse_a11y::DEFAULT_NETWORK_BUFFER_CAPACITY,
        )
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("{REQUESTS_TOOL} raw CDP network capture failed: {error}"),
            )
        })?;
        let read = synapse_a11y::network_capture_read(
            cdp_target_id,
            &synapse_a11y::CdpNetworkReadFilter {
                since_seq: filters.since_seq,
                max: 0,
                ..Default::default()
            },
        )
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("{REQUESTS_TOOL} network capture was not armed for target {cdp_target_id}"),
            )
        })?;
        let entries = filter_network_entries(read.entries.into_iter(), filters)
            .into_iter()
            .take(filters.limit)
            .map(|entry| browser_network_entry_to_wire(&entry))
            .collect::<Vec<_>>();
        tracing::info!(
            code = "CDP_BACKGROUND_NETWORK_REQUESTS",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            returned = entries.len(),
            total_buffered = read.total_buffered,
            next_cursor = read.next_cursor,
            "readback=Network.event_buffer(browser_network_requests) outcome=list_returned"
        );
        Ok(BrowserNetworkRequestsResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            capture_newly_armed: capture.newly_armed,
            next_cursor: read.next_cursor,
            returned: entries.len(),
            total_buffered: read.total_buffered,
            dropped: read.dropped,
            filters: filters.to_wire(),
            entries,
            readback_backend: "Network event buffer(browser_network_requests)".to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        })
    }

    #[cfg(not(windows))]
    pub(super) async fn browser_network_requests_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _filters: &NormalizedBrowserNetworkRequestsParams,
    ) -> Result<BrowserNetworkRequestsResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_network_requests is only available on Windows in this build",
        ))
    }

    #[cfg(windows)]
    pub(super) async fn browser_network_websockets_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        filters: &NormalizedBrowserNetworkWebSocketsParams,
    ) -> Result<BrowserNetworkWebSocketsResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(WEBSOCKETS_TOOL, window_hwnd));
        };
        let capture = synapse_a11y::network_capture_ensure(
            &endpoint,
            cdp_target_id,
            synapse_a11y::DEFAULT_NETWORK_BUFFER_CAPACITY,
        )
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("{WEBSOCKETS_TOOL} raw CDP network capture failed: {error}"),
            )
        })?;
        let read = synapse_a11y::network_web_socket_read(
            cdp_target_id,
            &synapse_a11y::CdpWebSocketReadFilter {
                since_seq: filters.since_seq,
                request_id: filters.request_id.as_deref(),
                url_contains: filters.url_contains.as_deref(),
                max: filters.limit,
            },
        )
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!(
                    "{WEBSOCKETS_TOOL} network capture was not armed for target {cdp_target_id}"
                ),
            )
        })?;
        let entries = read
            .entries
            .iter()
            .map(browser_network_websocket_entry_to_wire)
            .collect::<Vec<_>>();
        tracing::info!(
            code = "CDP_BACKGROUND_NETWORK_WEBSOCKETS",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            returned = entries.len(),
            total_buffered = read.total_buffered,
            next_cursor = read.next_cursor,
            "readback=Network.webSocket* event_buffer(browser_network_websockets) outcome=list_returned"
        );
        Ok(BrowserNetworkWebSocketsResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            capture_newly_armed: capture.newly_armed,
            next_cursor: read.next_cursor,
            returned: entries.len(),
            total_buffered: read.total_buffered,
            dropped: read.dropped,
            filters: filters.to_wire(),
            entries,
            readback_backend: "Network.webSocket* event buffer(browser_network_websockets)"
                .to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        })
    }

    #[cfg(not(windows))]
    pub(super) async fn browser_network_websockets_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _filters: &NormalizedBrowserNetworkWebSocketsParams,
    ) -> Result<BrowserNetworkWebSocketsResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_network_websockets is only available on Windows in this build",
        ))
    }

    #[cfg(windows)]
    pub(super) async fn browser_network_request_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        request: &NormalizedBrowserNetworkRequestParams,
    ) -> Result<BrowserNetworkRequestResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(REQUEST_TOOL, window_hwnd));
        };
        let capture = synapse_a11y::network_capture_ensure(
            &endpoint,
            cdp_target_id,
            synapse_a11y::DEFAULT_NETWORK_BUFFER_CAPACITY,
        )
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("{REQUEST_TOOL} raw CDP network capture failed: {error}"),
            )
        })?;
        let read = synapse_a11y::network_capture_read(
            cdp_target_id,
            &synapse_a11y::CdpNetworkReadFilter {
                request_id: Some(request.request_id.as_str()),
                max: 1,
                ..Default::default()
            },
        )
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("{REQUEST_TOOL} network capture was not armed for target {cdp_target_id}"),
            )
        })?;
        let Some(entry) = read.entries.into_iter().next() else {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{REQUEST_TOOL} request_id {:?} is not present in the target network buffer",
                    request.request_id
                ),
            ));
        };
        let request_post_data = if request.include_post_data
            && entry.request_has_post_data.unwrap_or(false)
        {
            Some(
                synapse_a11y::network_request_post_data(cdp_target_id, &entry.request_id)
                    .await
                    .map_err(|error| {
                        mcp_error(
                            error.code(),
                            format!(
                                "{REQUEST_TOOL} raw CDP request post data failed request_id={}: {error}",
                                entry.request_id
                            ),
                        )
                    })
                    .map(browser_network_post_data_to_wire)?,
            )
        } else {
            None
        };
        let response_body = if request.include_body {
            require_response_body_available(&entry)?;
            Some(
                synapse_a11y::network_response_body(cdp_target_id, &entry.request_id)
                    .await
                    .map_err(|error| {
                        mcp_error(
                            error.code(),
                            format!(
                                "{REQUEST_TOOL} raw CDP response body failed request_id={}: {error}",
                                entry.request_id
                            ),
                        )
                    })
                    .map(browser_network_response_body_to_wire)?,
            )
        } else {
            None
        };
        tracing::info!(
            code = "CDP_BACKGROUND_NETWORK_REQUEST",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            request_id = %entry.request_id,
            include_body = request.include_body,
            response_body_returned = response_body.is_some(),
            request_post_data_returned = request_post_data.is_some(),
            "readback=Network.getResponseBody(browser_network_request) outcome=request_returned"
        );
        Ok(BrowserNetworkRequestResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            capture_newly_armed: capture.newly_armed,
            request_id: entry.request_id.clone(),
            include_body: request.include_body,
            include_post_data: request.include_post_data,
            entry: browser_network_request_detail_to_wire(&entry),
            request_post_data,
            response_body,
            readback_backend:
                "Network event buffer + Network.getResponseBody(browser_network_request)".to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        })
    }

    #[cfg(not(windows))]
    pub(super) async fn browser_network_request_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _request: &NormalizedBrowserNetworkRequestParams,
    ) -> Result<BrowserNetworkRequestResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_network_request is only available on Windows in this build",
        ))
    }
}
