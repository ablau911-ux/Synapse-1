//! Raw CDP implementation for `browser_network_har`.

use super::HAR_TOOL;
use super::har_io::{
    clear_har_replay_routes, load_har_replay, stop_fetch_if_no_routes, write_har_record,
};
use super::normalized::NormalizedBrowserNetworkHarParams;
use super::types_har_overrides::{BrowserNetworkHarOperation, BrowserNetworkHarResponse};
use super::validation_requests::filter_network_entries;
use super::wire::{browser_route_fetch_status_from_a11y, browser_route_rule_to_wire};
use crate::m1::mcp_error;
use crate::server::m1_tools::browser_raw_cdp_required_error;
use crate::server::{ErrorData, SynapseService};
use synapse_core::error_codes;

impl SynapseService {
    #[cfg(windows)]
    pub(super) async fn browser_network_har_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        har: &NormalizedBrowserNetworkHarParams,
    ) -> Result<BrowserNetworkHarResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(HAR_TOOL, window_hwnd));
        };

        let mut capture_newly_armed = false;
        let mut recorded_entry_count = 0usize;
        let mut skipped_entry_count = 0usize;
        let mut replay_entry_count = 0usize;
        let mut replay_route_count = 0usize;
        let mut cleared_replay_route_count = 0usize;
        let mut missing_abort_route_installed = false;
        let mut har_bytes = 0u64;

        match har.operation {
            BrowserNetworkHarOperation::Record => {
                let capture = synapse_a11y::network_capture_ensure(
                    &endpoint,
                    cdp_target_id,
                    synapse_a11y::DEFAULT_NETWORK_BUFFER_CAPACITY,
                )
                .await
                .map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!("{HAR_TOOL} raw CDP network capture failed: {error}"),
                    )
                })?;
                capture_newly_armed = capture.newly_armed;
                let read = synapse_a11y::network_capture_read(
                    cdp_target_id,
                    &synapse_a11y::CdpNetworkReadFilter {
                        since_seq: har.filters.since_seq,
                        max: 0,
                        ..Default::default()
                    },
                )
                .ok_or_else(|| {
                    mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!(
                            "{HAR_TOOL} network capture was not armed for target {cdp_target_id}"
                        ),
                    )
                })?;
                let entries = filter_network_entries(read.entries.into_iter(), &har.filters)
                    .into_iter()
                    .take(har.filters.limit)
                    .collect::<Vec<_>>();
                let path = har.path.as_ref().ok_or_else(|| {
                    mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!("{HAR_TOOL} record path was not normalized"),
                    )
                })?;
                let record = write_har_record(
                    cdp_target_id,
                    path,
                    &entries,
                    har.include_bodies,
                    har.overwrite,
                )
                .await?;
                recorded_entry_count = record.entry_count;
                skipped_entry_count = record.skipped_count;
                har_bytes = record.bytes_written;
            }
            BrowserNetworkHarOperation::Replay => {
                let path = har.path.as_ref().ok_or_else(|| {
                    mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!("{HAR_TOOL} replay path was not normalized"),
                    )
                })?;
                if har.clear_existing_replay {
                    cleared_replay_route_count = clear_har_replay_routes(cdp_target_id)?;
                }
                let replay = load_har_replay(path, har.filters.limit, har.missing_policy)?;
                har_bytes = replay.source_bytes;
                replay_entry_count = replay.entry_count;
                skipped_entry_count = replay.skipped_count;
                replay_route_count = replay.rules.len();
                missing_abort_route_installed = replay.missing_abort_route_installed;

                let ensure =
                    synapse_a11y::fetch_interception_ensure(&endpoint, cdp_target_id, Vec::new())
                        .await
                        .map_err(|error| {
                            mcp_error(
                                error.code(),
                                format!("{HAR_TOOL} raw CDP Fetch interception failed: {error}"),
                            )
                        })?;
                for rule in replay.rules {
                    let mut status =
                        synapse_a11y::fetch_route_add(cdp_target_id, rule).map_err(|error| {
                            mcp_error(
                                error.code(),
                                format!("{HAR_TOOL} raw CDP Fetch route add failed: {error}"),
                            )
                        })?;
                    status.newly_armed = ensure.newly_armed;
                }
            }
            BrowserNetworkHarOperation::ClearReplay => {
                cleared_replay_route_count = clear_har_replay_routes(cdp_target_id)?;
                stop_fetch_if_no_routes(cdp_target_id).await?;
            }
        }

        let route_rules = synapse_a11y::fetch_route_rules(cdp_target_id).unwrap_or_default();
        let fetch_status_raw = synapse_a11y::fetch_interception_status(cdp_target_id);
        let fetch_status = browser_route_fetch_status_from_a11y(
            fetch_status_raw,
            synapse_a11y::fetch_interception_status(cdp_target_id).is_some(),
        );
        let routes = route_rules
            .iter()
            .map(browser_route_rule_to_wire)
            .collect::<Vec<_>>();

        tracing::info!(
            code = "CDP_BACKGROUND_NETWORK_HAR",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            operation = ?har.operation,
            recorded_entry_count,
            skipped_entry_count,
            replay_entry_count,
            replay_route_count,
            cleared_replay_route_count,
            missing_abort_route_installed,
            "readback=HAR record/replay(browser_network_har) outcome=har_operation_returned"
        );

        Ok(BrowserNetworkHarResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            operation: har.operation,
            path: har.path_display.clone(),
            filters: (har.operation == BrowserNetworkHarOperation::Record)
                .then(|| har.to_record_filters_wire()),
            missing_policy: (har.operation == BrowserNetworkHarOperation::Replay)
                .then_some(har.missing_policy),
            capture_newly_armed,
            recorded_entry_count,
            skipped_entry_count,
            replay_entry_count,
            replay_route_count,
            cleared_replay_route_count,
            missing_abort_route_installed,
            har_bytes,
            route_count: routes.len(),
            routes,
            fetch_status,
            readback_backend: match har.operation {
                BrowserNetworkHarOperation::Record => {
                    "Network event buffer + Network.getResponseBody(browser_network_har)"
                }
                BrowserNetworkHarOperation::Replay | BrowserNetworkHarOperation::ClearReplay => {
                    "HAR file + Fetch fulfill routes(browser_network_har)"
                }
            }
            .to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        })
    }

    #[cfg(not(windows))]
    pub(super) async fn browser_network_har_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _har: &NormalizedBrowserNetworkHarParams,
    ) -> Result<BrowserNetworkHarResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_network_har is only available on Windows in this build",
        ))
    }
}
