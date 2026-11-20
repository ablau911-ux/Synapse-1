//! `browser_network*` / `browser_route` MCP tool entry points.

use super::types_har_overrides::{
    BrowserNetworkHarParams, BrowserNetworkHarResponse, BrowserNetworkOverridesParams,
    BrowserNetworkOverridesResponse,
};
use super::types_read::{
    BrowserNetworkParams, BrowserNetworkReadMode, BrowserNetworkRequestParams,
    BrowserNetworkRequestResponse, BrowserNetworkRequestsParams, BrowserNetworkRequestsResponse,
    BrowserNetworkResponse, BrowserNetworkWebSocketsParams, BrowserNetworkWebSocketsResponse,
};
use super::types_route::{BrowserRouteParams, BrowserRouteResponse};
use super::validation_har::validate_browser_network_har_params;
use super::validation_overrides::validate_browser_network_overrides_params;
use super::validation_requests::{
    validate_browser_network_request_params, validate_browser_network_requests_params,
    validate_browser_network_websockets_params,
};
use super::validation_route::validate_browser_route_params;
use super::{HAR_TOOL, OVERRIDES_TOOL, REQUEST_TOOL, REQUESTS_TOOL, ROUTE_TOOL, WEBSOCKETS_TOOL};
use crate::m1::mcp_error;
use crate::server::m1_tools::{cdp_target_id_audit_ref, require_target_session_id};
use crate::server::{ErrorData, Json, Parameters, SynapseService, tool, tool_router};
use rmcp::RoleServer;
use rmcp::service::RequestContext;
use serde_json::json;
use synapse_core::error_codes;

#[tool_router(router = browser_network_tool_router, vis = "pub(in crate::server)")]
impl SynapseService {
    #[tool(
        description = "Read captured network activity for the calling session's owned browser tab, selected by `mode` with the matching nested spec (#1348 — folds the former browser_network_requests/_request/_websockets read tools into one). mode=requests returns the filtered Network request-record list (spec `requests`: since_seq/limit/url_contains/url_regex/resource_type/status_min/status_max; all optional, omit the spec to list everything); mode=request inspects one captured request by CDP request_id with full request/response metadata, optional post data, and a base64-aware response body by default (spec `request`, required — carries request_id/include_body/include_post_data); mode=websockets returns WebSocket lifecycle and sent/received frame records (spec `websockets`: since_seq/limit/request_id/url_contains). All modes arm/reuse the same target-scoped raw CDP Network buffer. Target-scoped and background-safe: never activates the tab, never uses OS foreground input, never falls back to the human foreground tab. Raw CDP only; the popup-safe normal Chrome extension bridge fails closed. The response field matching `mode` carries that read's full result."
    )]
    pub async fn browser_network(
        &self,
        params: Parameters<BrowserNetworkParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserNetworkResponse>, ErrorData> {
        let params = params.0;
        let mode = params.mode;
        match mode {
            BrowserNetworkReadMode::Requests => {
                let spec = params.requests.unwrap_or_default();
                let inner = self
                    .browser_network_requests_inner(Parameters(spec), request_context)
                    .await?;
                Ok(Json(BrowserNetworkResponse {
                    mode,
                    requests: Some(inner.0),
                    ..Default::default()
                }))
            }
            BrowserNetworkReadMode::Request => {
                let spec = params.request.ok_or_else(|| {
                    mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        "browser_network mode=request requires the `request` spec object (with request_id)",
                    )
                })?;
                let inner = self
                    .browser_network_request_inner(Parameters(spec), request_context)
                    .await?;
                Ok(Json(BrowserNetworkResponse {
                    mode,
                    request: Some(inner.0),
                    ..Default::default()
                }))
            }
            BrowserNetworkReadMode::WebSockets => {
                let spec = params.websockets.unwrap_or_default();
                let inner = self
                    .browser_network_websockets_inner(Parameters(spec), request_context)
                    .await?;
                Ok(Json(BrowserNetworkResponse {
                    mode,
                    websockets: Some(inner.0),
                    ..Default::default()
                }))
            }
        }
    }

    /// List captured Network request records — internal lane for the unified
    /// `browser_network` tool (#1348, mode=requests).
    pub async fn browser_network_requests_inner(
        &self,
        params: Parameters<BrowserNetworkRequestsParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserNetworkRequestsResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = REQUESTS_TOOL,
            "tool.invocation kind=browser_network_requests"
        );
        let session_id = require_target_session_id(&request_context)?;
        let filters = validate_browser_network_requests_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "since_seq": filters.since_seq,
            "limit": filters.limit,
            "url_contains_len": filters.url_contains.as_deref().map(str::len),
            "url_regex_len": filters.url_regex_pattern.as_deref().map(str::len),
            "resource_type": filters.resource_type.as_deref(),
            "status_min": filters.status_min,
            "status_max": filters.status_max,
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            REQUESTS_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            REQUESTS_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "since_seq": filters.since_seq,
            "limit": filters.limit,
            "url_contains_len": filters.url_contains.as_deref().map(str::len),
            "url_regex_len": filters.url_regex_pattern.as_deref().map(str::len),
            "resource_type": filters.resource_type.as_deref(),
            "status_min": filters.status_min,
            "status_max": filters.status_max,
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            REQUESTS_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_network_requests_impl(&session_id, window_hwnd, &cdp_target_id, &filters)
            .await;
        self.audit_action_result_for_session(REQUESTS_TOOL, &result, &session_id)?;
        result.map(Json)
    }

    /// List captured WebSocket lifecycle/frame records — internal lane for the
    /// unified `browser_network` tool (#1348, mode=websockets).
    pub async fn browser_network_websockets_inner(
        &self,
        params: Parameters<BrowserNetworkWebSocketsParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserNetworkWebSocketsResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = WEBSOCKETS_TOOL,
            "tool.invocation kind=browser_network_websockets"
        );
        let session_id = require_target_session_id(&request_context)?;
        let filters = validate_browser_network_websockets_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "since_seq": filters.since_seq,
            "limit": filters.limit,
            "request_id": filters.request_id.as_deref(),
            "url_contains_len": filters.url_contains.as_deref().map(str::len),
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            WEBSOCKETS_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            WEBSOCKETS_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "since_seq": filters.since_seq,
            "limit": filters.limit,
            "request_id": filters.request_id.as_deref(),
            "url_contains_len": filters.url_contains.as_deref().map(str::len),
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            WEBSOCKETS_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_network_websockets_impl(&session_id, window_hwnd, &cdp_target_id, &filters)
            .await;
        self.audit_action_result_for_session(WEBSOCKETS_TOOL, &result, &session_id)?;
        result.map(Json)
    }

    #[tool(
        description = "Record captured Network requests to a HAR 1.2 file, replay a HAR through exact target-scoped Fetch fulfill routes, or clear HAR replay routes for the calling session's owned browser tab. Record reuses the raw CDP Network buffer and can include retained Network.getResponseBody payloads. Replay reads a local HAR file, installs exact URL fulfill routes, and makes the missing-entry policy explicit as passthrough or abort. Target-scoped and background-safe: never activates the tab, never uses OS foreground input, and never falls back to the human foreground tab. Raw CDP only; the popup-safe normal Chrome bridge fails closed."
    )]
    pub async fn browser_network_har(
        &self,
        params: Parameters<BrowserNetworkHarParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserNetworkHarResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = HAR_TOOL,
            "tool.invocation kind=browser_network_har"
        );
        let session_id = require_target_session_id(&request_context)?;
        let har = validate_browser_network_har_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "operation": har.operation,
            "path": har.path_display.as_deref(),
            "since_seq": har.filters.since_seq,
            "limit": har.filters.limit,
            "url_contains_len": har.filters.url_contains.as_deref().map(str::len),
            "url_regex_len": har.filters.url_regex_pattern.as_deref().map(str::len),
            "resource_type": har.filters.resource_type.as_deref(),
            "status_min": har.filters.status_min,
            "status_max": har.filters.status_max,
            "include_bodies": har.include_bodies,
            "overwrite": har.overwrite,
            "missing_policy": har.missing_policy,
            "clear_existing_replay": har.clear_existing_replay,
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            HAR_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            HAR_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "operation": har.operation,
            "path": har.path_display.as_deref(),
            "since_seq": har.filters.since_seq,
            "limit": har.filters.limit,
            "url_contains_len": har.filters.url_contains.as_deref().map(str::len),
            "url_regex_len": har.filters.url_regex_pattern.as_deref().map(str::len),
            "resource_type": har.filters.resource_type.as_deref(),
            "status_min": har.filters.status_min,
            "status_max": har.filters.status_max,
            "include_bodies": har.include_bodies,
            "overwrite": har.overwrite,
            "missing_policy": har.missing_policy,
            "clear_existing_replay": har.clear_existing_replay,
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            HAR_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_network_har_impl(&session_id, window_hwnd, &cdp_target_id, &har)
            .await;
        self.audit_action_result_for_session(HAR_TOOL, &result, &session_id)?;
        result.map(Json)
    }

    /// Inspect one captured Network request by id — internal lane for the
    /// unified `browser_network` tool (#1348, mode=request).
    pub async fn browser_network_request_inner(
        &self,
        params: Parameters<BrowserNetworkRequestParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserNetworkRequestResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = REQUEST_TOOL,
            "tool.invocation kind=browser_network_request"
        );
        let session_id = require_target_session_id(&request_context)?;
        let request = validate_browser_network_request_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "request_id": &request.request_id,
            "include_body": request.include_body,
            "include_post_data": request.include_post_data,
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            REQUEST_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            REQUEST_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "request_id": &request.request_id,
            "include_body": request.include_body,
            "include_post_data": request.include_post_data,
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            REQUEST_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_network_request_impl(&session_id, window_hwnd, &cdp_target_id, &request)
            .await;
        self.audit_action_result_for_session(REQUEST_TOOL, &result, &session_id)?;
        result.map(Json)
    }

    #[tool(
        description = "Set/get/clear target-scoped extra HTTP headers and User-Agent override for the calling session's owned browser tab. Uses raw CDP Network.setExtraHTTPHeaders and Emulation.setUserAgentOverride, keeps a target-scoped override session alive for readback/clear, and never activates the tab, uses OS foreground input, or falls back to the human foreground tab. Raw CDP only; the popup-safe normal Chrome extension bridge fails closed."
    )]
    pub async fn browser_network_overrides(
        &self,
        params: Parameters<BrowserNetworkOverridesParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserNetworkOverridesResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = OVERRIDES_TOOL,
            "tool.invocation kind=browser_network_overrides"
        );
        let session_id = require_target_session_id(&request_context)?;
        let overrides = validate_browser_network_overrides_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "operation": overrides.operation,
            "header_count": overrides.headers.len(),
            "user_agent_len": overrides.user_agent.as_deref().map(str::len),
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            OVERRIDES_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            OVERRIDES_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "operation": overrides.operation,
            "header_count": overrides.headers.len(),
            "user_agent_len": overrides.user_agent.as_deref().map(str::len),
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            OVERRIDES_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_network_overrides_impl(&session_id, window_hwnd, &cdp_target_id, &overrides)
            .await;
        self.audit_action_result_for_session(OVERRIDES_TOOL, &result, &session_id)?;
        result.map(Json)
    }

    #[tool(
        description = "Add/list/remove/clear Fetch route rules for the calling session's owned browser tab. The default add_fulfill operation arms target-scoped raw CDP Fetch interception and fulfills matching URL glob/regex requests with status/headers/body; add_abort fails matching requests with Fetch.failRequest and a CDP Network.ErrorReason; add_continue continues matching requests with optional URL, method, headers, and postData overrides. Unmatched requests continue by default. Target-scoped and background-safe: never activates the tab, never uses OS foreground input, and never falls back to the human foreground tab. Raw CDP only; the popup-safe normal Chrome extension bridge fails closed."
    )]
    pub async fn browser_route(
        &self,
        params: Parameters<BrowserRouteParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserRouteResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = ROUTE_TOOL,
            "tool.invocation kind=browser_route"
        );
        let session_id = require_target_session_id(&request_context)?;
        let route = validate_browser_route_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "operation": route.operation,
            "route_id": route.route_id.as_deref(),
            "url_len": params.0.url.as_deref().map(str::len),
            "match_kind": params.0.match_kind,
            "resource_type": params.0.resource_type.as_deref(),
            "status": params.0.status,
            "header_count": params.0.headers.len(),
            "body_len": params.0.body.as_deref().map(str::len),
            "body_base64_len": params.0.body_base64.as_deref().map(str::len),
            "error_reason": params.0.error_reason,
            "continue_url_len": params.0.continue_url.as_deref().map(str::len),
            "continue_method": params.0.continue_method.as_deref(),
            "continue_header_count": params.0.continue_headers.len(),
            "continue_post_data_len": params.0.continue_post_data.as_deref().map(str::len),
            "continue_post_data_base64_len": params.0.continue_post_data_base64.as_deref().map(str::len),
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            ROUTE_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            ROUTE_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "operation": route.operation,
            "route_id": route.route_id.as_deref(),
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            ROUTE_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_route_impl(&session_id, window_hwnd, &cdp_target_id, &route)
            .await;
        self.audit_action_result_for_session(ROUTE_TOOL, &result, &session_id)?;
        result.map(Json)
    }
}
