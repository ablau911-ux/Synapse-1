//! Raw CDP implementations for network overrides and `browser_route`.

use super::normalized::{NormalizedBrowserNetworkOverridesParams, NormalizedBrowserRouteParams};
use super::types_har_overrides::{
    BrowserNetworkOverridesOperation, BrowserNetworkOverridesResponse,
};
use super::types_route::{BrowserRouteOperation, BrowserRouteResponse};
use super::wire::{
    browser_network_overrides_response, browser_route_fetch_status_from_a11y,
    browser_route_rule_to_wire,
};
use super::{OVERRIDES_TOOL, ROUTE_TOOL};
use crate::m1::mcp_error;
use crate::server::m1_tools::browser_raw_cdp_required_error;
use crate::server::{ErrorData, SynapseService};
use synapse_core::error_codes;

impl SynapseService {
    #[cfg(windows)]
    pub(super) async fn browser_network_overrides_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        overrides: &NormalizedBrowserNetworkOverridesParams,
    ) -> Result<BrowserNetworkOverridesResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(OVERRIDES_TOOL, window_hwnd));
        };
        let (status, cleared) = match overrides.operation {
            BrowserNetworkOverridesOperation::Set => {
                let status = synapse_a11y::network_overrides_apply(
                    &endpoint,
                    cdp_target_id,
                    synapse_a11y::CdpNetworkOverrideConfig {
                        headers: overrides.headers.clone(),
                        user_agent: overrides.user_agent.clone(),
                    },
                )
                .await
                .map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!("{OVERRIDES_TOOL} raw CDP override apply failed: {error}"),
                    )
                })?;
                (Some(status), false)
            }
            BrowserNetworkOverridesOperation::Get => {
                (synapse_a11y::network_overrides_status(cdp_target_id), false)
            }
            BrowserNetworkOverridesOperation::Clear => {
                let status = synapse_a11y::network_overrides_clear(cdp_target_id)
                    .await
                    .map_err(|error| {
                        mcp_error(
                            error.code(),
                            format!("{OVERRIDES_TOOL} raw CDP override clear failed: {error}"),
                        )
                    })?;
                let cleared = status.is_some();
                (status, cleared)
            }
        };
        tracing::info!(
            code = "CDP_BACKGROUND_NETWORK_OVERRIDES",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            operation = ?overrides.operation,
            override_active = status.is_some(),
            cleared,
            "readback=Network.setExtraHTTPHeaders+Emulation.setUserAgentOverride outcome=overrides_returned"
        );
        Ok(browser_network_overrides_response(
            session_id,
            window_hwnd,
            endpoint,
            cdp_target_id,
            overrides.operation,
            status,
            cleared,
        ))
    }

    #[cfg(not(windows))]
    pub(super) async fn browser_network_overrides_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _overrides: &NormalizedBrowserNetworkOverridesParams,
    ) -> Result<BrowserNetworkOverridesResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_network_overrides is only available on Windows in this build",
        ))
    }

    #[cfg(windows)]
    pub(super) async fn browser_route_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        route: &NormalizedBrowserRouteParams,
    ) -> Result<BrowserRouteResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(ROUTE_TOOL, window_hwnd));
        };
        let mut route_removed = false;
        let mut cleared_count = 0usize;
        let fetch_status = match route.operation {
            BrowserRouteOperation::AddFulfill
            | BrowserRouteOperation::AddAbort
            | BrowserRouteOperation::AddContinue => {
                let ensure =
                    synapse_a11y::fetch_interception_ensure(&endpoint, cdp_target_id, Vec::new())
                        .await
                        .map_err(|error| {
                            mcp_error(
                                error.code(),
                                format!("{ROUTE_TOOL} raw CDP Fetch interception failed: {error}"),
                            )
                        })?;
                let normalized_route = route.route.clone().ok_or_else(|| {
                    mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!("{ROUTE_TOOL} add_fulfill route was not normalized"),
                    )
                })?;
                let mut status = synapse_a11y::fetch_route_add(cdp_target_id, normalized_route)
                    .map_err(|error| {
                        mcp_error(
                            error.code(),
                            format!("{ROUTE_TOOL} raw CDP Fetch route add failed: {error}"),
                        )
                    })?;
                status.newly_armed = ensure.newly_armed;
                browser_route_fetch_status_from_a11y(Some(status), true)
            }
            BrowserRouteOperation::Remove => {
                if let Some(route_id) = route.route_id.as_deref() {
                    route_removed = synapse_a11y::fetch_route_remove(cdp_target_id, route_id)
                        .map_err(|error| {
                            mcp_error(
                                error.code(),
                                format!("{ROUTE_TOOL} raw CDP Fetch route remove failed: {error}"),
                            )
                        })?;
                }
                let routes = synapse_a11y::fetch_route_rules(cdp_target_id).unwrap_or_default();
                let status = synapse_a11y::fetch_interception_status(cdp_target_id);
                if routes.is_empty() && status.is_some() {
                    synapse_a11y::fetch_interception_stop(cdp_target_id)
                        .await
                        .map_err(|error| {
                            mcp_error(
                                error.code(),
                                format!("{ROUTE_TOOL} raw CDP Fetch disable failed: {error}"),
                            )
                        })?;
                    browser_route_fetch_status_from_a11y(None, false)
                } else {
                    browser_route_fetch_status_from_a11y(status, !routes.is_empty())
                }
            }
            BrowserRouteOperation::Clear => {
                cleared_count =
                    synapse_a11y::fetch_route_clear(cdp_target_id).map_err(|error| {
                        mcp_error(
                            error.code(),
                            format!("{ROUTE_TOOL} raw CDP Fetch route clear failed: {error}"),
                        )
                    })?;
                if synapse_a11y::fetch_interception_status(cdp_target_id).is_some() {
                    synapse_a11y::fetch_interception_stop(cdp_target_id)
                        .await
                        .map_err(|error| {
                            mcp_error(
                                error.code(),
                                format!("{ROUTE_TOOL} raw CDP Fetch disable failed: {error}"),
                            )
                        })?;
                }
                browser_route_fetch_status_from_a11y(None, false)
            }
            BrowserRouteOperation::List => {
                let status = synapse_a11y::fetch_interception_status(cdp_target_id);
                let fetch_armed = status.is_some();
                browser_route_fetch_status_from_a11y(status, fetch_armed)
            }
        };
        let routes = synapse_a11y::fetch_route_rules(cdp_target_id)
            .unwrap_or_default()
            .iter()
            .map(browser_route_rule_to_wire)
            .collect::<Vec<_>>();
        tracing::info!(
            code = "CDP_BACKGROUND_BROWSER_ROUTE",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            operation = ?route.operation,
            route_id = route.route_id.as_deref(),
            route_count = routes.len(),
            route_removed,
            cleared_count,
            "readback=Fetch.fulfillRequest(browser_route) outcome=route_operation_returned"
        );
        Ok(BrowserRouteResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            operation: route.operation,
            route_id: route.route_id.clone(),
            route_removed,
            cleared_count,
            route_count: routes.len(),
            routes,
            fetch_status,
            readback_backend: "Fetch interception routes(browser_route)".to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        })
    }

    #[cfg(not(windows))]
    pub(super) async fn browser_route_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _route: &NormalizedBrowserRouteParams,
    ) -> Result<BrowserRouteResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_route is only available on Windows in this build",
        ))
    }
}
