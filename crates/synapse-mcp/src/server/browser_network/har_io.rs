//! HAR record/replay file IO and HAR-to-route-rule conversion.

use super::har_redact::{har_header_value_for_storage, is_redacted_har_header};
use super::har_types::{
    HarContent, HarCreator, HarEntry, HarFile, HarHeader, HarLog, HarPostData,
    HarRecordWriteResult, HarReplayLoadResult, HarRequest, HarResponse, HarTimings,
    har_http_version, har_version,
};
use super::types_har_overrides::BrowserNetworkHarMissingPolicy;
use super::types_route::BrowserRouteErrorReason;
use super::{HAR_REPLAY_MISS_ROUTE_ID, HAR_REPLAY_ROUTE_PREFIX, HAR_TOOL, MAX_HAR_FILE_BYTES};
use crate::m1::mcp_error;
use crate::server::ErrorData;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_core::error_codes;

pub(super) async fn write_har_record(
    cdp_target_id: &str,
    path: &PathBuf,
    entries: &[synapse_a11y::CdpNetworkEntry],
    include_bodies: bool,
    overwrite: bool,
) -> Result<HarRecordWriteResult, ErrorData> {
    if !overwrite && path.exists() {
        return Err(har_exists_error(path));
    }
    let mut har_entries = Vec::new();
    let mut skipped_count = 0usize;
    for entry in entries {
        let response_body = if include_bodies && entry.response_received && entry.loading_finished {
            synapse_a11y::network_response_body(cdp_target_id, &entry.request_id)
                .await
                .ok()
        } else {
            None
        };
        let request_post_data = if include_bodies && entry.request_has_post_data == Some(true) {
            synapse_a11y::network_request_post_data(cdp_target_id, &entry.request_id)
                .await
                .ok()
        } else {
            None
        };
        if let Some(har_entry) = har_entry_from_network(entry, response_body, request_post_data) {
            har_entries.push(har_entry);
        } else {
            skipped_count = skipped_count.saturating_add(1);
        }
    }

    let har = HarFile {
        log: HarLog {
            version: har_version(),
            creator: HarCreator::default(),
            entries: har_entries,
        },
    };
    let bytes = serde_json::to_vec_pretty(&har).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("{HAR_TOOL} failed to serialize HAR: {error}"),
        )
    })?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("{HAR_TOOL} failed to create HAR parent directory: {error}"),
            )
        })?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                har_exists_error(path)
            } else {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("{HAR_TOOL} failed to write HAR {}: {error}", path.display()),
                )
            }
        })?;
    Ok(HarRecordWriteResult {
        entry_count: har.log.entries.len(),
        skipped_count,
        bytes_written: bytes.len() as u64,
    })
}

fn har_exists_error(path: &Path) -> ErrorData {
    mcp_error(
        error_codes::TOOL_PARAMS_INVALID,
        format!(
            "{HAR_TOOL} refuses to overwrite existing file {}; pass overwrite=true to replace it",
            path.display()
        ),
    )
}

pub(super) fn load_har_replay(
    path: &PathBuf,
    limit: usize,
    missing_policy: BrowserNetworkHarMissingPolicy,
) -> Result<HarReplayLoadResult, ErrorData> {
    let metadata = fs::metadata(path).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{HAR_TOOL} failed to stat HAR {}: {error}", path.display()),
        )
    })?;
    if metadata.len() > MAX_HAR_FILE_BYTES {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{HAR_TOOL} HAR file must be at most {MAX_HAR_FILE_BYTES} bytes"),
        ));
    }
    let raw = fs::read_to_string(path).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{HAR_TOOL} failed to read HAR {}: {error}", path.display()),
        )
    })?;
    let har: HarFile = serde_json::from_str(&raw).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{HAR_TOOL} failed to parse HAR JSON: {error}"),
        )
    })?;
    let mut rules = Vec::new();
    let mut entry_count = 0usize;
    let mut skipped_count = 0usize;
    for (index, entry) in har.log.entries.iter().take(limit).enumerate() {
        match har_entry_to_route_rule(index, entry)? {
            Some(rule) => {
                entry_count = entry_count.saturating_add(1);
                rules.push(rule);
            }
            None => skipped_count = skipped_count.saturating_add(1),
        }
    }
    let mut missing_abort_route_installed = false;
    if missing_policy == BrowserNetworkHarMissingPolicy::Abort {
        rules.push(har_missing_abort_rule());
        missing_abort_route_installed = true;
    }
    Ok(HarReplayLoadResult {
        entry_count,
        skipped_count,
        source_bytes: metadata.len(),
        missing_abort_route_installed,
        rules,
    })
}

fn har_entry_from_network(
    entry: &synapse_a11y::CdpNetworkEntry,
    response_body: Option<synapse_a11y::CdpNetworkResponseBody>,
    request_post_data: Option<synapse_a11y::CdpNetworkRequestPostData>,
) -> Option<HarEntry> {
    let url = entry.url.clone()?;
    let method = entry.method.clone().unwrap_or_else(|| "GET".to_owned());
    let response = entry.response.as_ref();
    let started_date_time = har_datetime(entry.request_wall_time_ms);
    let time = match (entry.request_timestamp_s, entry.finished_timestamp_s) {
        (Some(start), Some(finish)) if finish >= start => (finish - start) * 1000.0,
        _ => -1.0,
    };
    let post_data = request_post_data.map(|post_data| HarPostData {
        mime_type: header_lookup(entry.request_headers.as_ref(), "content-type")
            .unwrap_or_default(),
        text: post_data.post_data,
        params: Vec::new(),
    });
    let response_status = response.map_or(0, |response| response.status);
    let response_status_text = response
        .map(|response| response.status_text.clone())
        .unwrap_or_default();
    let response_protocol = response
        .and_then(|response| response.protocol.clone())
        .unwrap_or_else(har_http_version);
    let response_headers = response
        .map(|response| har_headers_from_value(Some(&response.headers)))
        .unwrap_or_default();
    let content = har_content_from_response(response, response_body);
    let body_size = content.size;
    Some(HarEntry {
        started_date_time,
        time,
        request: HarRequest {
            method,
            url,
            http_version: har_http_version(),
            cookies: Vec::new(),
            headers: har_headers_from_value(entry.request_headers.as_ref()),
            query_string: Vec::new(),
            post_data,
            headers_size: -1,
            body_size: -1,
        },
        response: HarResponse {
            status: response_status,
            status_text: response_status_text,
            http_version: response_protocol,
            cookies: Vec::new(),
            headers: response_headers,
            content,
            redirect_url: String::new(),
            headers_size: -1,
            body_size,
        },
        cache: json!({}),
        timings: HarTimings::default(),
        synapse_request_id: Some(entry.request_id.clone()),
        synapse_resource_type: entry.resource_type.clone(),
    })
}

pub(super) fn har_entry_to_route_rule(
    index: usize,
    entry: &HarEntry,
) -> Result<Option<synapse_a11y::CdpFetchRouteRule>, ErrorData> {
    if entry.request.url.is_empty()
        || entry.request.method.is_empty()
        || !(100..=599).contains(&entry.response.status)
    {
        return Ok(None);
    }
    let body_base64 = har_content_body_base64(&entry.response.content)?;
    Ok(Some(synapse_a11y::CdpFetchRouteRule {
        id: har_route_id(index, &entry.request.method, &entry.request.url),
        url: format!("^{}$", regex::escape(&entry.request.url)),
        match_kind: synapse_a11y::CdpFetchRouteMatchKind::Regex,
        method: Some(entry.request.method.clone()),
        resource_type: None,
        action: synapse_a11y::CdpFetchRouteAction::Fulfill(synapse_a11y::CdpFetchRouteFulfill {
            status: entry.response.status,
            response_phrase: (!entry.response.status_text.is_empty())
                .then(|| entry.response.status_text.clone()),
            headers: entry
                .response
                .headers
                .iter()
                .filter(|header| !har_replay_unsafe_response_header(&header.name))
                .filter(|header| !is_redacted_har_header(&header.name, &header.value))
                .map(|header| (header.name.clone(), header.value.clone()))
                .collect(),
            body_base64,
        }),
    }))
}

fn har_replay_unsafe_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "content-encoding"
            | "content-length"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn har_missing_abort_rule() -> synapse_a11y::CdpFetchRouteRule {
    synapse_a11y::CdpFetchRouteRule {
        id: HAR_REPLAY_MISS_ROUTE_ID.to_owned(),
        url: "*".to_owned(),
        match_kind: synapse_a11y::CdpFetchRouteMatchKind::Glob,
        method: None,
        resource_type: None,
        action: synapse_a11y::CdpFetchRouteAction::Abort(synapse_a11y::CdpFetchRouteAbort {
            error_reason: BrowserRouteErrorReason::BlockedByClient
                .as_cdp_str()
                .to_owned(),
        }),
    }
}

pub(super) fn clear_har_replay_routes(cdp_target_id: &str) -> Result<usize, ErrorData> {
    let routes = synapse_a11y::fetch_route_rules(cdp_target_id).unwrap_or_default();
    let route_ids = routes
        .into_iter()
        .filter(|route| route.id.starts_with(HAR_REPLAY_ROUTE_PREFIX))
        .map(|route| route.id)
        .collect::<Vec<_>>();
    let mut removed = 0usize;
    for route_id in route_ids {
        if synapse_a11y::fetch_route_remove(cdp_target_id, &route_id).map_err(|error| {
            mcp_error(
                error.code(),
                format!("{HAR_TOOL} raw CDP Fetch route remove failed: {error}"),
            )
        })? {
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

pub(super) async fn stop_fetch_if_no_routes(cdp_target_id: &str) -> Result<(), ErrorData> {
    let routes = synapse_a11y::fetch_route_rules(cdp_target_id).unwrap_or_default();
    if routes.is_empty() && synapse_a11y::fetch_interception_status(cdp_target_id).is_some() {
        synapse_a11y::fetch_interception_stop(cdp_target_id)
            .await
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("{HAR_TOOL} raw CDP Fetch disable failed: {error}"),
                )
            })?;
    }
    Ok(())
}

fn har_headers_from_value(value: Option<&Value>) -> Vec<HarHeader> {
    let Some(Value::Object(headers)) = value else {
        return Vec::new();
    };
    headers
        .iter()
        .map(|(name, value)| HarHeader {
            name: name.clone(),
            value: har_header_value_for_storage(name, header_json_value_to_string(value)),
        })
        .collect()
}

fn header_lookup(headers: Option<&Value>, name: &str) -> Option<String> {
    let Value::Object(headers) = headers? else {
        return None;
    };
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| header_json_value_to_string(value))
}

fn header_json_value_to_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn har_content_from_response(
    response: Option<&synapse_a11y::CdpNetworkResponseSnapshot>,
    body: Option<synapse_a11y::CdpNetworkResponseBody>,
) -> HarContent {
    let mime_type = response
        .map(|response| response.mime_type.clone())
        .unwrap_or_default();
    let mut content = HarContent {
        size: response
            .map(|response| response.encoded_data_length.max(0.0) as i64)
            .unwrap_or(0),
        mime_type,
        text: None,
        encoding: None,
    };
    if let Some(body) = body {
        content.size = if body.base64_encoded {
            BASE64_STANDARD
                .decode(&body.body)
                .map(|bytes| bytes.len() as i64)
                .unwrap_or_else(|_| body.body.len() as i64)
        } else {
            body.body.len() as i64
        };
        content.text = Some(body.body);
        content.encoding = body.base64_encoded.then(|| "base64".to_owned());
    }
    content
}

fn har_content_body_base64(content: &HarContent) -> Result<Option<String>, ErrorData> {
    let Some(text) = content.text.as_deref() else {
        return Ok(None);
    };
    if content
        .encoding
        .as_deref()
        .is_some_and(|encoding| encoding.eq_ignore_ascii_case("base64"))
    {
        BASE64_STANDARD.decode(text).map_err(|error| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{HAR_TOOL} HAR response content is not valid base64: {error}"),
            )
        })?;
        return Ok(Some(text.to_owned()));
    }
    Ok(Some(BASE64_STANDARD.encode(text.as_bytes())))
}

fn har_route_id(index: usize, method: &str, url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    method.hash(&mut hasher);
    url.hash(&mut hasher);
    format!(
        "{HAR_REPLAY_ROUTE_PREFIX}{index:04x}-{:016x}",
        hasher.finish()
    )
}

fn har_datetime(unix_ms: Option<f64>) -> String {
    let unix_ms = unix_ms.unwrap_or_else(current_unix_ms);
    let secs = (unix_ms / 1000.0).floor() as i64;
    let nanos = ((secs as f64).mul_add(-1000.0, unix_ms) * 1_000_000.0)
        .round()
        .clamp(0.0, 999_999_999.0) as u32;
    DateTime::<Utc>::from_timestamp(secs, nanos)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn current_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}
