//! HAR file serde types and their default helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug)]
pub(super) struct HarRecordWriteResult {
    pub(super) entry_count: usize,
    pub(super) skipped_count: usize,
    pub(super) bytes_written: u64,
}

#[derive(Debug)]
pub(super) struct HarReplayLoadResult {
    pub(super) entry_count: usize,
    pub(super) skipped_count: usize,
    pub(super) source_bytes: u64,
    pub(super) missing_abort_route_installed: bool,
    pub(super) rules: Vec<synapse_a11y::CdpFetchRouteRule>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarFile {
    pub(super) log: HarLog,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarLog {
    #[serde(default = "har_version")]
    pub(super) version: String,
    #[serde(default)]
    pub(super) creator: HarCreator,
    #[serde(default)]
    pub(super) entries: Vec<HarEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct HarCreator {
    pub(super) name: String,
    pub(super) version: String,
}

impl Default for HarCreator {
    fn default() -> Self {
        Self {
            name: "Synapse".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarEntry {
    #[serde(rename = "startedDateTime", default = "har_epoch_datetime")]
    pub(super) started_date_time: String,
    #[serde(default = "har_unknown_time")]
    pub(super) time: f64,
    #[serde(default)]
    pub(super) request: HarRequest,
    #[serde(default)]
    pub(super) response: HarResponse,
    #[serde(default)]
    pub(super) cache: Value,
    #[serde(default)]
    pub(super) timings: HarTimings,
    #[serde(
        rename = "_synapseRequestId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) synapse_request_id: Option<String>,
    #[serde(
        rename = "_synapseResourceType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) synapse_resource_type: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarRequest {
    #[serde(default)]
    pub(super) method: String,
    #[serde(default)]
    pub(super) url: String,
    #[serde(rename = "httpVersion", default = "har_http_version")]
    pub(super) http_version: String,
    #[serde(default)]
    pub(super) cookies: Vec<Value>,
    #[serde(default)]
    pub(super) headers: Vec<HarHeader>,
    #[serde(rename = "queryString", default)]
    pub(super) query_string: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) post_data: Option<HarPostData>,
    #[serde(rename = "headersSize", default = "har_size_unknown")]
    pub(super) headers_size: i64,
    #[serde(rename = "bodySize", default = "har_size_unknown")]
    pub(super) body_size: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarPostData {
    #[serde(rename = "mimeType", default)]
    pub(super) mime_type: String,
    #[serde(default)]
    pub(super) text: String,
    #[serde(default)]
    pub(super) params: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarResponse {
    #[serde(default)]
    pub(super) status: i64,
    #[serde(rename = "statusText", default)]
    pub(super) status_text: String,
    #[serde(rename = "httpVersion", default = "har_http_version")]
    pub(super) http_version: String,
    #[serde(default)]
    pub(super) cookies: Vec<Value>,
    #[serde(default)]
    pub(super) headers: Vec<HarHeader>,
    #[serde(default)]
    pub(super) content: HarContent,
    #[serde(rename = "redirectURL", default)]
    pub(super) redirect_url: String,
    #[serde(rename = "headersSize", default = "har_size_unknown")]
    pub(super) headers_size: i64,
    #[serde(rename = "bodySize", default = "har_size_unknown")]
    pub(super) body_size: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarHeader {
    pub(super) name: String,
    pub(super) value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HarContent {
    #[serde(default)]
    pub(super) size: i64,
    #[serde(rename = "mimeType", default)]
    pub(super) mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) encoding: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct HarTimings {
    pub(super) send: f64,
    pub(super) wait: f64,
    pub(super) receive: f64,
}

impl Default for HarTimings {
    fn default() -> Self {
        Self {
            send: 0.0,
            wait: 0.0,
            receive: 0.0,
        }
    }
}

pub(super) fn har_version() -> String {
    "1.2".to_owned()
}

pub(super) fn har_http_version() -> String {
    "HTTP/2".to_owned()
}

fn har_epoch_datetime() -> String {
    "1970-01-01T00:00:00.000Z".to_owned()
}

fn har_unknown_time() -> f64 {
    -1.0
}

fn har_size_unknown() -> i64 {
    -1
}
