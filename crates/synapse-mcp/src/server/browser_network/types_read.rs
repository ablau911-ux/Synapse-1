//! Request-list, request-detail and WebSocket params/response wire types.

use crate::m1::BrowserNetworkWaitEntry;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which captured-network read the unified `browser_network` tool returns
/// (#1348). Supply the matching nested spec under [`BrowserNetworkParams`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkReadMode {
    /// List captured Network request records (filterable).
    #[default]
    Requests,
    /// Inspect one captured Network request by CDP request id.
    Request,
    /// List captured WebSocket lifecycle/frame records.
    #[serde(rename = "websockets")]
    WebSockets,
}

/// Parameters for the unified `browser_network` tool (#1348): one `mode`
/// discriminator and the matching nested spec, each reused verbatim from the
/// former standalone browser_network_requests/_request/_websockets tools. The
/// requests/websockets specs default to an empty (unfiltered) read when omitted;
/// the request spec is required because it carries the mandatory `request_id`.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkParams {
    /// Which captured-network read to perform.
    pub mode: BrowserNetworkReadMode,
    /// `mode=requests`: filtered request-record list.
    #[serde(default)]
    pub requests: Option<BrowserNetworkRequestsParams>,
    /// `mode=request`: single request inspection (requires `request_id`).
    #[serde(default)]
    pub request: Option<BrowserNetworkRequestParams>,
    /// `mode=websockets`: WebSocket lifecycle/frame list.
    #[serde(default)]
    pub websockets: Option<BrowserNetworkWebSocketsParams>,
}

/// Response for the unified `browser_network` tool (#1348): the populated field
/// matches `mode` and carries the former standalone tool's full response.
#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkResponse {
    /// Which captured-network read was performed.
    pub mode: BrowserNetworkReadMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<BrowserNetworkRequestsResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<BrowserNetworkRequestResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websockets: Option<BrowserNetworkWebSocketsResponse>,
}

/// Parameters for `browser_network_requests` (#1081): return captured Network
/// request records for the calling session's owned CDP target.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestsParams {
    /// CDP TargetID to read. Defaults to the active session CDP target. Must be
    /// owned by this session; the human foreground tab is never an implicit fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// Return only records whose latest update sequence is >= this cursor.
    #[serde(default)]
    pub since_seq: Option<u64>,
    /// Maximum records to return after filtering. Defaults to 100, max 1000.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Case-insensitive substring filter against request URL.
    #[serde(default)]
    pub url_contains: Option<String>,
    /// Regular expression filter against request URL.
    #[serde(default)]
    pub url_regex: Option<String>,
    /// Case-insensitive CDP Network resource type filter.
    #[serde(default)]
    pub resource_type: Option<String>,
    /// Minimum HTTP status, inclusive.
    #[serde(default)]
    pub status_min: Option<i64>,
    /// Maximum HTTP status, inclusive.
    #[serde(default)]
    pub status_max: Option<i64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestFilters {
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
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestsResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub capture_newly_armed: bool,
    pub next_cursor: u64,
    pub returned: usize,
    pub total_buffered: usize,
    pub dropped: u64,
    pub filters: BrowserNetworkRequestFilters,
    pub entries: Vec<BrowserNetworkWaitEntry>,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
}

/// Parameters for `browser_network_websockets` (#1089): return captured
/// WebSocket lifecycle and sent/received frame records for the calling
/// session's owned CDP target.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkWebSocketsParams {
    /// CDP TargetID to read. Defaults to the active session CDP target. Must be
    /// owned by this session; the human foreground tab is never an implicit fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// Return only sockets whose latest update sequence is >= this cursor.
    #[serde(default)]
    pub since_seq: Option<u64>,
    /// Maximum sockets to return after filtering. Defaults to 100, max 1000.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Exact CDP request id filter.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Case-insensitive substring filter against WebSocket URL.
    #[serde(default)]
    pub url_contains: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkWebSocketFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_contains: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkWebSocketsResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub capture_newly_armed: bool,
    pub next_cursor: u64,
    pub returned: usize,
    pub total_buffered: usize,
    pub dropped: u64,
    pub filters: BrowserNetworkWebSocketFilters,
    pub entries: Vec<BrowserNetworkWebSocketEntry>,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
}

/// Parameters for `browser_network_request` (#1082): inspect one captured
/// Network request by CDP request id, including response body by default.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestParams {
    /// CDP request id from `browser_network_requests`, `browser_wait_for_request`,
    /// or `browser_wait_for_response`.
    pub request_id: String,
    /// CDP TargetID to read. Defaults to the active session CDP target. Must be
    /// owned by this session; the human foreground tab is never an implicit fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// Include `Network.getResponseBody` readback. Defaults to true.
    #[serde(default = "default_true")]
    pub include_body: bool,
    /// Include `Network.getRequestPostData` when CDP reported post data.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub include_post_data: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub capture_newly_armed: bool,
    pub request_id: String,
    pub include_body: bool,
    pub include_post_data: bool,
    pub entry: BrowserNetworkRequestDetail,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_post_data: Option<BrowserNetworkRequestPostData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<BrowserNetworkResponseBody>,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestDetail {
    pub seq: u64,
    pub first_seq: u64,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_has_post_data: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_wall_time_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<Value>,
    pub redirects: Vec<BrowserNetworkResponseSnapshot>,
    pub response_received: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<BrowserNetworkResponseSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_timestamp_s: Option<f64>,
    pub loading_finished: bool,
    pub loading_failed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoded_data_length: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_error_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_canceled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_cors_error_status: Option<Value>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkResponseSnapshot {
    pub url: String,
    pub status: i64,
    pub status_text: String,
    pub headers: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Value>,
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_port: Option<i64>,
    pub encoded_data_length: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_disk_cache: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_service_worker: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_prefetch_cache: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_early_hints: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestPostData {
    pub request_id: String,
    pub post_data: String,
    pub post_data_len_chars: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkResponseBody {
    pub request_id: String,
    pub body: String,
    pub base64_encoded: bool,
    pub body_len_chars: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkWebSocketEntry {
    pub seq: u64,
    pub first_seq: u64,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_request_timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_request_wall_time_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_request_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_response_timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_response_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_response_headers_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_response_request_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_response_request_headers_text: Option<String>,
    pub frames: Vec<BrowserNetworkWebSocketFrame>,
    pub sent_frame_count: u64,
    pub received_frame_count: u64,
    pub frame_error_count: u64,
    pub dropped_frames: u64,
    pub closed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkWebSocketFrame {
    pub seq: u64,
    pub direction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opcode: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_data: Option<String>,
    pub payload_len_chars: usize,
    pub payload_base64_encoded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

fn default_true() -> bool {
    true
}
