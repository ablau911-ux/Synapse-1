//! HAR and network-override params/response wire types.

use super::types_route::{BrowserRouteFetchStatus, BrowserRouteHeader, BrowserRouteRuleResponse};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Operation for `browser_network_har` (#1088).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkHarOperation {
    /// Serialize captured Network entries to a HAR 1.2 file.
    #[default]
    Record,
    /// Install Fetch fulfill rules from a HAR file.
    Replay,
    /// Remove route rules previously installed by HAR replay.
    ClearReplay,
}

/// Missing-entry policy for HAR replay.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkHarMissingPolicy {
    /// Requests absent from the HAR continue to the network.
    #[default]
    Passthrough,
    /// Requests absent from the HAR fail with `BlockedByClient`.
    Abort,
}

/// Parameters for `browser_network_har` (#1088): record captured requests to a
/// HAR file or replay a HAR through target-scoped Fetch fulfill routes.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkHarParams {
    /// CDP TargetID to record/replay. Defaults to the active session CDP target.
    /// Must be owned by this session; the human foreground tab is never an
    /// implicit fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// HAR operation. Defaults to `record`.
    #[serde(default)]
    pub operation: BrowserNetworkHarOperation,
    /// HAR file path (absolute or relative, must end in `.har`; UNC, device,
    /// alternate-data-stream and reserved-name paths are rejected). Required
    /// for `record` and `replay`.
    #[serde(default)]
    pub path: Option<String>,
    /// Record only entries whose latest update sequence is >= this cursor.
    #[serde(default)]
    pub since_seq: Option<u64>,
    /// Maximum entries to record/replay. Defaults to 100 for record and all HAR
    /// entries for replay, capped at 1000.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Case-insensitive substring filter against request URL while recording.
    #[serde(default)]
    pub url_contains: Option<String>,
    /// Regular expression filter against request URL while recording.
    #[serde(default)]
    pub url_regex: Option<String>,
    /// Case-insensitive CDP Network resource type filter while recording.
    #[serde(default)]
    pub resource_type: Option<String>,
    /// Minimum HTTP status, inclusive, while recording.
    #[serde(default)]
    pub status_min: Option<i64>,
    /// Maximum HTTP status, inclusive, while recording.
    #[serde(default)]
    pub status_max: Option<i64>,
    /// Include retained request/response bodies in recorded HAR content.
    /// Defaults false: bodies can hold credentials and personal data.
    #[serde(default)]
    pub include_bodies: Option<bool>,
    /// Allow `record` to replace an existing file at `path`. Defaults false:
    /// an existing file is never overwritten without this explicit opt-in.
    #[serde(default)]
    pub overwrite: Option<bool>,
    /// Replay behavior for requests not present in the HAR. Defaults
    /// `passthrough`.
    #[serde(default)]
    pub missing_policy: Option<BrowserNetworkHarMissingPolicy>,
    /// Remove existing HAR replay rules for this target before replaying.
    /// Defaults true.
    #[serde(default)]
    pub clear_existing_replay: Option<bool>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkHarFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_max: Option<i64>,
    pub include_bodies: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkHarResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: BrowserNetworkHarOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<BrowserNetworkHarFilters>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_policy: Option<BrowserNetworkHarMissingPolicy>,
    pub capture_newly_armed: bool,
    pub recorded_entry_count: usize,
    pub skipped_entry_count: usize,
    pub replay_entry_count: usize,
    pub replay_route_count: usize,
    pub cleared_replay_route_count: usize,
    pub missing_abort_route_installed: bool,
    pub har_bytes: u64,
    pub route_count: usize,
    pub routes: Vec<BrowserRouteRuleResponse>,
    pub fetch_status: BrowserRouteFetchStatus,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
}

/// Operation for `browser_network_overrides` (#1087).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkOverridesOperation {
    /// Replace target-scoped headers and optional User-Agent override.
    #[default]
    Set,
    /// Read current tracked override state.
    Get,
    /// Clear headers and restore the captured original User-Agent.
    Clear,
}

/// Parameters for `browser_network_overrides` (#1087).
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkOverridesParams {
    /// CDP TargetID to configure. Defaults to the active session CDP target.
    /// Must be owned by this session; the human foreground tab is never an implicit fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// Operation. Defaults to `set`.
    #[serde(default)]
    pub operation: BrowserNetworkOverridesOperation,
    /// Replacement extra HTTP headers for `set`. An empty list clears headers.
    #[serde(default)]
    pub headers: Vec<BrowserRouteHeader>,
    /// Replacement User-Agent for `set`. Omit to clear a prior UA override.
    #[serde(default)]
    pub user_agent: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkOverridesResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: BrowserNetworkOverridesOperation,
    pub override_active: bool,
    pub newly_armed: bool,
    pub cleared: bool,
    pub armed_at_unix_ms: u64,
    pub applied_at_unix_ms: u64,
    pub header_count: usize,
    pub headers: Vec<BrowserRouteHeader>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_user_agent: Option<String>,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
}
