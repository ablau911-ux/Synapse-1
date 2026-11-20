//! `browser_route` params/response wire types.

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Operation for `browser_route` (#1084).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRouteOperation {
    /// Add or replace a route that fulfills matching requests.
    #[default]
    AddFulfill,
    /// Add or replace a route that aborts matching requests.
    AddAbort,
    /// Add or replace a route that continues matching requests with overrides.
    AddContinue,
    /// Remove one route by id.
    Remove,
    /// Clear all routes for the target and disable Fetch interception.
    Clear,
    /// List active routes without arming interception.
    List,
}

/// URL match kind for `browser_route` (#1084).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRouteMatchKind {
    /// Simple glob with `*` and `?`.
    #[default]
    Glob,
    /// Rust regular expression.
    Regex,
}

/// CDP network error reason for `browser_route` add_abort (#1085).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRouteErrorReason {
    Failed,
    Aborted,
    TimedOut,
    AccessDenied,
    ConnectionClosed,
    ConnectionReset,
    ConnectionRefused,
    ConnectionAborted,
    ConnectionFailed,
    NameNotResolved,
    InternetDisconnected,
    AddressUnreachable,
    #[default]
    BlockedByClient,
    BlockedByResponse,
}

impl BrowserRouteErrorReason {
    pub(super) fn as_cdp_str(self) -> &'static str {
        match self {
            Self::Failed => "Failed",
            Self::Aborted => "Aborted",
            Self::TimedOut => "TimedOut",
            Self::AccessDenied => "AccessDenied",
            Self::ConnectionClosed => "ConnectionClosed",
            Self::ConnectionReset => "ConnectionReset",
            Self::ConnectionRefused => "ConnectionRefused",
            Self::ConnectionAborted => "ConnectionAborted",
            Self::ConnectionFailed => "ConnectionFailed",
            Self::NameNotResolved => "NameNotResolved",
            Self::InternetDisconnected => "InternetDisconnected",
            Self::AddressUnreachable => "AddressUnreachable",
            Self::BlockedByClient => "BlockedByClient",
            Self::BlockedByResponse => "BlockedByResponse",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrowserRouteHeader {
    pub name: String,
    pub value: String,
}

/// Parameters for `browser_route` (#1084): add/list/remove/clear target-scoped
/// Fetch routes. `add_fulfill` fulfills matching requests; `add_abort` fails
/// matching requests; unmatched requests continue by default.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserRouteParams {
    /// CDP TargetID to route. Defaults to the active session CDP target. Must
    /// be owned by this session; the human foreground tab is never an implicit fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// Route operation. Defaults to `add_fulfill`.
    #[serde(default)]
    pub operation: BrowserRouteOperation,
    /// Route id. Optional for `add_fulfill`; generated when omitted.
    #[serde(default)]
    pub route_id: Option<String>,
    /// URL glob or regex for `add_fulfill`.
    #[serde(default)]
    pub url: Option<String>,
    /// URL match kind for `add_fulfill`. Defaults to `glob`.
    #[serde(default)]
    pub match_kind: BrowserRouteMatchKind,
    /// Optional CDP resource type, e.g. `Document`, `XHR`, `Fetch`, `Script`.
    #[serde(default)]
    pub resource_type: Option<String>,
    /// HTTP status for fulfilled responses. Defaults to 200.
    #[serde(default)]
    pub status: Option<i64>,
    /// Optional reason phrase for the fulfilled response.
    #[serde(default)]
    pub response_phrase: Option<String>,
    /// UTF-8 response headers for the fulfilled response.
    #[serde(default)]
    pub headers: Vec<BrowserRouteHeader>,
    /// UTF-8 response body. Mutually exclusive with `body_base64`.
    #[serde(default)]
    pub body: Option<String>,
    /// Base64-encoded response body. Mutually exclusive with `body`.
    #[serde(default)]
    pub body_base64: Option<String>,
    /// CDP Network.ErrorReason for `add_abort`. Defaults to `blocked_by_client`.
    #[serde(default)]
    pub error_reason: Option<BrowserRouteErrorReason>,
    /// Replacement request URL for `add_continue`.
    #[serde(default)]
    pub continue_url: Option<String>,
    /// Replacement request method for `add_continue`.
    #[serde(default)]
    pub continue_method: Option<String>,
    /// Replacement request headers for `add_continue`.
    #[serde(default)]
    pub continue_headers: Vec<BrowserRouteHeader>,
    /// UTF-8 replacement request postData for `add_continue`.
    #[serde(default)]
    pub continue_post_data: Option<String>,
    /// Base64 replacement request postData for `add_continue`.
    #[serde(default)]
    pub continue_post_data_base64: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserRouteResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: BrowserRouteOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    pub route_removed: bool,
    pub cleared_count: usize,
    pub route_count: usize,
    pub routes: Vec<BrowserRouteRuleResponse>,
    pub fetch_status: BrowserRouteFetchStatus,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserRouteRuleResponse {
    pub id: String,
    pub url: String,
    pub match_kind: BrowserRouteMatchKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_phrase: Option<String>,
    pub headers: Vec<BrowserRouteHeader>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64_len_chars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_data_base64_len_chars: Option<usize>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserRouteFetchStatus {
    pub fetch_armed: bool,
    pub newly_armed: bool,
    pub armed_at_unix_ms: u64,
    pub pattern_count: usize,
    pub route_count: usize,
    pub paused_count: u64,
    pub continued_count: u64,
    pub fulfilled_count: u64,
    pub failed_count: u64,
    pub continue_error_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_route_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
