//! Network capture listing tools (#1081) backed by the a11y CDP Network buffer.

const REQUESTS_TOOL: &str = "browser_network_requests";
const REQUEST_TOOL: &str = "browser_network_request";
const WEBSOCKETS_TOOL: &str = "browser_network_websockets";
const HAR_TOOL: &str = "browser_network_har";
const OVERRIDES_TOOL: &str = "browser_network_overrides";
const ROUTE_TOOL: &str = "browser_route";
const DEFAULT_NETWORK_REQUEST_LIMIT: usize = 100;
const MAX_NETWORK_REQUEST_LIMIT: usize = 1000;
const MAX_NETWORK_FILTER_CHARS: usize = 8192;
const MAX_NETWORK_RESOURCE_TYPE_CHARS: usize = 128;
const MAX_NETWORK_REQUEST_ID_CHARS: usize = 2048;
const MAX_ROUTE_ID_CHARS: usize = 256;
const MAX_ROUTE_URL_CHARS: usize = 8192;
const MAX_ROUTE_RESPONSE_PHRASE_CHARS: usize = 256;
const MAX_ROUTE_HEADER_COUNT: usize = 128;
const MAX_ROUTE_HEADER_NAME_CHARS: usize = 256;
const MAX_ROUTE_HEADER_VALUE_CHARS: usize = 8192;
const MAX_ROUTE_BODY_CHARS: usize = 1_048_576;
const MAX_NETWORK_USER_AGENT_CHARS: usize = 4096;
const MAX_HAR_REPLAY_ENTRIES: usize = 1000;
const MAX_HAR_FILE_BYTES: u64 = 64 * 1024 * 1024;
const HAR_REPLAY_ROUTE_PREFIX: &str = "har-replay-";
const HAR_REPLAY_MISS_ROUTE_ID: &str = "har-replay-missing";

mod har_io;
mod har_redact;
mod har_types;
mod impl_har;
mod impl_overrides_route;
mod impl_read;
mod normalized;
mod route_body;
#[cfg(test)]
mod tests_har_hardening_184;
#[cfg(test)]
mod tests_overrides_route;
#[cfg(test)]
mod tests_read;
#[cfg(test)]
mod tests_support;
mod tools;
mod types_har_overrides;
mod types_read;
mod types_route;
mod validation_har;
mod validation_har_path;
mod validation_overrides;
mod validation_requests;
mod validation_route;
mod wire;

pub use types_har_overrides::{
    BrowserNetworkHarParams, BrowserNetworkHarResponse, BrowserNetworkOverridesParams,
    BrowserNetworkOverridesResponse,
};
pub use types_read::{BrowserNetworkParams, BrowserNetworkResponse};
pub use types_route::{BrowserRouteParams, BrowserRouteResponse};
