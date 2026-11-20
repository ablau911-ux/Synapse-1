//! Normalized (validated) parameter structs shared by the validators and impls.

use super::types_har_overrides::{
    BrowserNetworkHarFilters, BrowserNetworkHarMissingPolicy, BrowserNetworkHarOperation,
    BrowserNetworkOverridesOperation,
};
use super::types_read::{BrowserNetworkRequestFilters, BrowserNetworkWebSocketFilters};
use super::types_route::BrowserRouteOperation;
use std::path::PathBuf;

#[derive(Debug)]
pub(super) struct NormalizedBrowserNetworkRequestsParams {
    pub(super) since_seq: Option<u64>,
    pub(super) limit: usize,
    pub(super) url_contains: Option<String>,
    pub(super) url_regex_pattern: Option<String>,
    pub(super) url_regex: Option<regex::Regex>,
    pub(super) resource_type: Option<String>,
    pub(super) status_min: Option<i64>,
    pub(super) status_max: Option<i64>,
}

#[derive(Debug)]
pub(super) struct NormalizedBrowserNetworkWebSocketsParams {
    pub(super) since_seq: Option<u64>,
    pub(super) limit: usize,
    pub(super) request_id: Option<String>,
    pub(super) url_contains: Option<String>,
}

#[derive(Debug)]
pub(super) struct NormalizedBrowserNetworkHarParams {
    pub(super) operation: BrowserNetworkHarOperation,
    pub(super) path: Option<PathBuf>,
    pub(super) path_display: Option<String>,
    pub(super) filters: NormalizedBrowserNetworkRequestsParams,
    pub(super) include_bodies: bool,
    pub(super) overwrite: bool,
    pub(super) missing_policy: BrowserNetworkHarMissingPolicy,
    pub(super) clear_existing_replay: bool,
}

#[derive(Debug)]
pub(super) struct NormalizedBrowserNetworkRequestParams {
    pub(super) request_id: String,
    pub(super) include_body: bool,
    pub(super) include_post_data: bool,
}

#[derive(Debug)]
pub(super) struct NormalizedBrowserNetworkOverridesParams {
    pub(super) operation: BrowserNetworkOverridesOperation,
    pub(super) headers: Vec<(String, String)>,
    pub(super) user_agent: Option<String>,
}

#[derive(Debug)]
pub(super) struct NormalizedBrowserRouteParams {
    pub(super) operation: BrowserRouteOperation,
    pub(super) route_id: Option<String>,
    pub(super) route: Option<synapse_a11y::CdpFetchRouteRule>,
}

impl NormalizedBrowserNetworkRequestsParams {
    pub(super) fn to_wire(&self) -> BrowserNetworkRequestFilters {
        BrowserNetworkRequestFilters {
            since_seq: self.since_seq,
            limit: self.limit,
            url_contains: self.url_contains.clone(),
            url_regex: self.url_regex_pattern.clone(),
            resource_type: self.resource_type.clone(),
            status_min: self.status_min,
            status_max: self.status_max,
        }
    }
}

impl NormalizedBrowserNetworkWebSocketsParams {
    pub(super) fn to_wire(&self) -> BrowserNetworkWebSocketFilters {
        BrowserNetworkWebSocketFilters {
            since_seq: self.since_seq,
            limit: self.limit,
            request_id: self.request_id.clone(),
            url_contains: self.url_contains.clone(),
        }
    }
}

impl NormalizedBrowserNetworkHarParams {
    pub(super) fn to_record_filters_wire(&self) -> BrowserNetworkHarFilters {
        BrowserNetworkHarFilters {
            since_seq: self.filters.since_seq,
            limit: self.filters.limit,
            url_contains: self.filters.url_contains.clone(),
            url_regex: self.filters.url_regex_pattern.clone(),
            resource_type: self.filters.resource_type.clone(),
            status_min: self.filters.status_min,
            status_max: self.filters.status_max,
            include_bodies: self.include_bodies,
        }
    }
}
