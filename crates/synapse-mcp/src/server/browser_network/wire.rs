//! Mapping from a11y CDP network types to MCP wire types.

use super::REQUEST_TOOL;
use super::types_har_overrides::{
    BrowserNetworkOverridesOperation, BrowserNetworkOverridesResponse,
};
use super::types_read::{
    BrowserNetworkRequestDetail, BrowserNetworkRequestPostData, BrowserNetworkResponseBody,
    BrowserNetworkResponseSnapshot, BrowserNetworkWebSocketEntry, BrowserNetworkWebSocketFrame,
};
use super::types_route::{
    BrowserRouteFetchStatus, BrowserRouteHeader, BrowserRouteMatchKind, BrowserRouteRuleResponse,
};
use crate::m1::{BrowserNetworkWaitEntry, mcp_error};
use crate::server::ErrorData;
use crate::server::url_redaction::{
    redact_url_for_public_readback, redact_url_opt_for_public_readback,
};
use synapse_core::error_codes;

pub(super) fn browser_network_entry_to_wire(
    entry: &synapse_a11y::CdpNetworkEntry,
) -> BrowserNetworkWaitEntry {
    let response = entry.response.as_ref();
    BrowserNetworkWaitEntry {
        seq: entry.seq,
        request_id: entry.request_id.clone(),
        url: redact_url_opt_for_public_readback(entry.url.clone()),
        method: entry.method.clone(),
        resource_type: entry.resource_type.clone(),
        request_headers: entry.request_headers.clone(),
        response_received: entry.response_received,
        response_url: response.map(|response| redact_url_for_public_readback(&response.url)),
        status: response.map(|response| response.status),
        status_text: response.map(|response| response.status_text.clone()),
        response_headers: response.map(|response| response.headers.clone()),
        response_timing: response.and_then(|response| response.timing.clone()),
        protocol: response.and_then(|response| response.protocol.clone()),
        remote_ip_address: response.and_then(|response| response.remote_ip_address.clone()),
        remote_port: response.and_then(|response| response.remote_port),
        encoded_data_length: entry
            .encoded_data_length
            .or_else(|| response.map(|response| response.encoded_data_length)),
        loading_finished: entry.loading_finished,
        loading_failed: entry.loading_failed,
        failure_error_text: entry.failure_error_text.clone(),
    }
}

pub(super) fn browser_network_request_detail_to_wire(
    entry: &synapse_a11y::CdpNetworkEntry,
) -> BrowserNetworkRequestDetail {
    BrowserNetworkRequestDetail {
        seq: entry.seq,
        first_seq: entry.first_seq,
        request_id: entry.request_id.clone(),
        loader_id: entry.loader_id.clone(),
        frame_id: entry.frame_id.clone(),
        document_url: redact_url_opt_for_public_readback(entry.document_url.clone()),
        url: redact_url_opt_for_public_readback(entry.url.clone()),
        method: entry.method.clone(),
        resource_type: entry.resource_type.clone(),
        request_headers: entry.request_headers.clone(),
        request_has_post_data: entry.request_has_post_data,
        request_timestamp_s: entry.request_timestamp_s,
        request_wall_time_ms: entry.request_wall_time_ms,
        initiator: entry.initiator.clone(),
        redirects: entry
            .redirects
            .iter()
            .map(browser_network_response_snapshot_to_wire)
            .collect(),
        response_received: entry.response_received,
        response: entry
            .response
            .as_ref()
            .map(browser_network_response_snapshot_to_wire),
        response_timestamp_s: entry.response_timestamp_s,
        loading_finished: entry.loading_finished,
        loading_failed: entry.loading_failed,
        finished_timestamp_s: entry.finished_timestamp_s,
        failed_timestamp_s: entry.failed_timestamp_s,
        encoded_data_length: entry.encoded_data_length,
        failure_error_text: entry.failure_error_text.clone(),
        failure_canceled: entry.failure_canceled,
        failure_blocked_reason: entry.failure_blocked_reason.clone(),
        failure_cors_error_status: entry.failure_cors_error_status.clone(),
    }
}

pub(super) fn browser_network_websocket_entry_to_wire(
    entry: &synapse_a11y::CdpWebSocketEntry,
) -> BrowserNetworkWebSocketEntry {
    BrowserNetworkWebSocketEntry {
        seq: entry.seq,
        first_seq: entry.first_seq,
        request_id: entry.request_id.clone(),
        url: redact_url_opt_for_public_readback(entry.url.clone()),
        created: entry.created,
        created_at_unix_ms: entry.created_at_unix_ms,
        initiator: entry.initiator.clone(),
        handshake_request_timestamp_s: entry.handshake_request_timestamp_s,
        handshake_request_wall_time_ms: entry.handshake_request_wall_time_ms,
        handshake_request_headers: entry.handshake_request_headers.clone(),
        handshake_response_timestamp_s: entry.handshake_response_timestamp_s,
        status: entry.status,
        status_text: entry.status_text.clone(),
        handshake_response_headers: entry.handshake_response_headers.clone(),
        handshake_response_headers_text: entry.handshake_response_headers_text.clone(),
        handshake_response_request_headers: entry.handshake_response_request_headers.clone(),
        handshake_response_request_headers_text: entry
            .handshake_response_request_headers_text
            .clone(),
        frames: entry
            .frames
            .iter()
            .map(browser_network_websocket_frame_to_wire)
            .collect(),
        sent_frame_count: entry.sent_frame_count,
        received_frame_count: entry.received_frame_count,
        frame_error_count: entry.frame_error_count,
        dropped_frames: entry.dropped_frames,
        closed: entry.closed,
        closed_timestamp_s: entry.closed_timestamp_s,
        close_code: entry.close_code,
        close_reason: entry.close_reason.clone(),
    }
}

fn browser_network_websocket_frame_to_wire(
    frame: &synapse_a11y::CdpWebSocketFrame,
) -> BrowserNetworkWebSocketFrame {
    BrowserNetworkWebSocketFrame {
        seq: frame.seq,
        direction: frame.direction.clone(),
        timestamp_s: frame.timestamp_s,
        opcode: frame.opcode,
        mask: frame.mask,
        payload_data: frame.payload_data.clone(),
        payload_len_chars: frame.payload_len_chars,
        payload_base64_encoded: frame.payload_base64_encoded,
        close_code: frame.close_code,
        close_reason: frame.close_reason.clone(),
        error_message: frame.error_message.clone(),
    }
}

fn browser_network_response_snapshot_to_wire(
    response: &synapse_a11y::CdpNetworkResponseSnapshot,
) -> BrowserNetworkResponseSnapshot {
    BrowserNetworkResponseSnapshot {
        url: redact_url_for_public_readback(&response.url),
        status: response.status,
        status_text: response.status_text.clone(),
        headers: response.headers.clone(),
        request_headers: response.request_headers.clone(),
        mime_type: response.mime_type.clone(),
        protocol: response.protocol.clone(),
        remote_ip_address: response.remote_ip_address.clone(),
        remote_port: response.remote_port,
        encoded_data_length: response.encoded_data_length,
        timing: response.timing.clone(),
        response_time_ms: response.response_time_ms,
        from_disk_cache: response.from_disk_cache,
        from_service_worker: response.from_service_worker,
        from_prefetch_cache: response.from_prefetch_cache,
        from_early_hints: response.from_early_hints,
        timestamp_s: response.timestamp_s,
        resource_type: response.resource_type.clone(),
    }
}

pub(super) fn browser_network_response_body_to_wire(
    body: synapse_a11y::CdpNetworkResponseBody,
) -> BrowserNetworkResponseBody {
    let body_len_chars = body.body.chars().count();
    BrowserNetworkResponseBody {
        request_id: body.request_id,
        body: body.body,
        base64_encoded: body.base64_encoded,
        body_len_chars,
    }
}

pub(super) fn browser_network_post_data_to_wire(
    post_data: synapse_a11y::CdpNetworkRequestPostData,
) -> BrowserNetworkRequestPostData {
    let post_data_len_chars = post_data.post_data.chars().count();
    BrowserNetworkRequestPostData {
        request_id: post_data.request_id,
        post_data: post_data.post_data,
        post_data_len_chars,
    }
}

pub(super) fn browser_network_overrides_response(
    session_id: &str,
    window_hwnd: i64,
    endpoint: String,
    cdp_target_id: &str,
    operation: BrowserNetworkOverridesOperation,
    status: Option<synapse_a11y::CdpNetworkOverrideStatus>,
    cleared: bool,
) -> BrowserNetworkOverridesResponse {
    match status {
        Some(status) => BrowserNetworkOverridesResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            operation,
            override_active: !status.headers.is_empty() || status.user_agent.is_some(),
            newly_armed: status.newly_armed,
            cleared,
            armed_at_unix_ms: status.armed_at_unix_ms,
            applied_at_unix_ms: status.applied_at_unix_ms,
            header_count: status.header_count,
            headers: status
                .headers
                .into_iter()
                .map(|(name, value)| BrowserRouteHeader { name, value })
                .collect(),
            user_agent: status.user_agent,
            original_user_agent: status.original_user_agent,
            readback_backend: "Network.setExtraHTTPHeaders + Emulation.setUserAgentOverride"
                .to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        },
        None => BrowserNetworkOverridesResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            operation,
            override_active: false,
            newly_armed: false,
            cleared,
            armed_at_unix_ms: 0,
            applied_at_unix_ms: 0,
            header_count: 0,
            headers: Vec::new(),
            user_agent: None,
            original_user_agent: None,
            readback_backend: "Network.setExtraHTTPHeaders + Emulation.setUserAgentOverride"
                .to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        },
    }
}

pub(super) fn require_response_body_available(
    entry: &synapse_a11y::CdpNetworkEntry,
) -> Result<(), ErrorData> {
    if !entry.response_received {
        return Err(mcp_error(
            error_codes::A11Y_CDP_AXTREE_FAILED,
            format!(
                "{REQUEST_TOOL} response body is unavailable for request_id={}: no responseReceived event captured",
                entry.request_id
            ),
        ));
    }
    if entry.loading_failed {
        return Err(mcp_error(
            error_codes::A11Y_CDP_AXTREE_FAILED,
            format!(
                "{REQUEST_TOOL} response body is unavailable for request_id={}: loadingFailed {:?}",
                entry.request_id, entry.failure_error_text
            ),
        ));
    }
    if !entry.loading_finished {
        return Err(mcp_error(
            error_codes::A11Y_CDP_AXTREE_FAILED,
            format!(
                "{REQUEST_TOOL} response body is unavailable for request_id={}: loadingFinished has not been captured yet",
                entry.request_id
            ),
        ));
    }
    Ok(())
}

pub(super) fn browser_route_fetch_status_from_a11y(
    status: Option<synapse_a11y::CdpFetchInterceptionStatus>,
    fetch_armed: bool,
) -> BrowserRouteFetchStatus {
    match status {
        Some(status) => BrowserRouteFetchStatus {
            fetch_armed,
            newly_armed: status.newly_armed,
            armed_at_unix_ms: status.armed_at_unix_ms,
            pattern_count: status.pattern_count,
            route_count: status.route_count,
            paused_count: status.paused_count,
            continued_count: status.continued_count,
            fulfilled_count: status.fulfilled_count,
            failed_count: status.failed_count,
            continue_error_count: status.continue_error_count,
            last_request_id: status.last_request_id,
            last_url: redact_url_opt_for_public_readback(status.last_url),
            last_route_id: status.last_route_id,
            last_error: status.last_error,
        },
        None => BrowserRouteFetchStatus {
            fetch_armed,
            newly_armed: false,
            armed_at_unix_ms: 0,
            pattern_count: 0,
            route_count: 0,
            paused_count: 0,
            continued_count: 0,
            fulfilled_count: 0,
            failed_count: 0,
            continue_error_count: 0,
            last_request_id: None,
            last_url: None,
            last_route_id: None,
            last_error: None,
        },
    }
}

pub(super) fn browser_route_rule_to_wire(
    rule: &synapse_a11y::CdpFetchRouteRule,
) -> BrowserRouteRuleResponse {
    match &rule.action {
        synapse_a11y::CdpFetchRouteAction::Fulfill(fulfill) => BrowserRouteRuleResponse {
            id: rule.id.clone(),
            url: redact_url_for_public_readback(&rule.url),
            match_kind: match rule.match_kind {
                synapse_a11y::CdpFetchRouteMatchKind::Glob => BrowserRouteMatchKind::Glob,
                synapse_a11y::CdpFetchRouteMatchKind::Regex => BrowserRouteMatchKind::Regex,
            },
            method: rule.method.clone(),
            resource_type: rule.resource_type.clone(),
            action: "fulfill".to_owned(),
            status: Some(fulfill.status),
            error_reason: None,
            continue_url: None,
            continue_method: None,
            response_phrase: fulfill.response_phrase.clone(),
            headers: fulfill
                .headers
                .iter()
                .map(|(name, value)| BrowserRouteHeader {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            body_base64_len_chars: fulfill
                .body_base64
                .as_ref()
                .map(|body| body.chars().count()),
            post_data_base64_len_chars: None,
        },
        synapse_a11y::CdpFetchRouteAction::Abort(abort) => BrowserRouteRuleResponse {
            id: rule.id.clone(),
            url: redact_url_for_public_readback(&rule.url),
            match_kind: match rule.match_kind {
                synapse_a11y::CdpFetchRouteMatchKind::Glob => BrowserRouteMatchKind::Glob,
                synapse_a11y::CdpFetchRouteMatchKind::Regex => BrowserRouteMatchKind::Regex,
            },
            method: rule.method.clone(),
            resource_type: rule.resource_type.clone(),
            action: "abort".to_owned(),
            status: None,
            error_reason: Some(abort.error_reason.clone()),
            continue_url: None,
            continue_method: None,
            response_phrase: None,
            headers: Vec::new(),
            body_base64_len_chars: None,
            post_data_base64_len_chars: None,
        },
        synapse_a11y::CdpFetchRouteAction::Continue(continue_rule) => BrowserRouteRuleResponse {
            id: rule.id.clone(),
            url: redact_url_for_public_readback(&rule.url),
            match_kind: match rule.match_kind {
                synapse_a11y::CdpFetchRouteMatchKind::Glob => BrowserRouteMatchKind::Glob,
                synapse_a11y::CdpFetchRouteMatchKind::Regex => BrowserRouteMatchKind::Regex,
            },
            method: rule.method.clone(),
            resource_type: rule.resource_type.clone(),
            action: "continue".to_owned(),
            status: None,
            error_reason: None,
            continue_url: redact_url_opt_for_public_readback(continue_rule.url.clone()),
            continue_method: continue_rule.method.clone(),
            response_phrase: None,
            headers: continue_rule
                .headers
                .iter()
                .map(|(name, value)| BrowserRouteHeader {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            body_base64_len_chars: None,
            post_data_base64_len_chars: continue_rule
                .post_data_base64
                .as_ref()
                .map(|post_data| post_data.chars().count()),
        },
    }
}
