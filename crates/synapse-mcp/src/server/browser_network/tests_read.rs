//! Tests for the request, requests, websockets and HAR paths.

use super::har_io::{har_entry_to_route_rule, load_har_replay, write_har_record};
use super::har_types::{HarContent, HarEntry, HarHeader, HarRequest, HarResponse};
use super::tests_support::{entry, websocket_entry};
use super::types_har_overrides::{
    BrowserNetworkHarMissingPolicy, BrowserNetworkHarOperation, BrowserNetworkHarParams,
};
use super::types_read::{
    BrowserNetworkRequestParams, BrowserNetworkRequestsParams, BrowserNetworkWebSocketsParams,
};
use super::validation_har::validate_browser_network_har_params;
use super::validation_requests::{
    filter_network_entries, validate_browser_network_request_params,
    validate_browser_network_requests_params, validate_browser_network_websockets_params,
};
use super::wire::{
    browser_network_entry_to_wire, browser_network_post_data_to_wire,
    browser_network_request_detail_to_wire, browser_network_response_body_to_wire,
    browser_network_websocket_entry_to_wire, require_response_body_available,
};
use super::{HAR_REPLAY_MISS_ROUTE_ID, MAX_NETWORK_REQUEST_LIMIT};
use serde_json::json;
use std::fs;
use synapse_core::error_codes;

#[test]
fn must_fail_browser_network_requests_validation_edges() {
    let ok = validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
        cdp_target_id: Some("target-123".to_owned()),
        since_seq: Some(7),
        limit: Some(MAX_NETWORK_REQUEST_LIMIT),
        url_regex: Some(r"^https://example\.test/api".to_owned()),
        resource_type: Some("XHR".to_owned()),
        status_min: Some(200),
        status_max: Some(299),
        ..Default::default()
    })
    .expect("valid params pass");
    assert_eq!(ok.since_seq, Some(7));
    assert_eq!(ok.limit, MAX_NETWORK_REQUEST_LIMIT);
    assert!(
        ok.url_regex
            .as_ref()
            .unwrap()
            .is_match("https://example.test/api")
    );
    assert_eq!(ok.resource_type.as_deref(), Some("XHR"));

    for error in [
        validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
            limit: Some(0),
            ..Default::default()
        })
        .expect_err("zero limit must be rejected"),
        validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
            url_contains: Some("api".to_owned()),
            url_regex: Some("api".to_owned()),
            ..Default::default()
        })
        .expect_err("ambiguous URL filters must be rejected"),
        validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
            url_regex: Some("(".to_owned()),
            ..Default::default()
        })
        .expect_err("invalid URL regex must be rejected"),
        validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
            resource_type: Some(" XHR".to_owned()),
            ..Default::default()
        })
        .expect_err("resource type whitespace must be rejected"),
        validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
            status_min: Some(500),
            status_max: Some(200),
            ..Default::default()
        })
        .expect_err("inverted status range must be rejected"),
    ] {
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID));
    }
}

#[test]
fn must_fail_browser_network_websockets_validation_edges() {
    let ok = validate_browser_network_websockets_params(&BrowserNetworkWebSocketsParams {
        cdp_target_id: Some("target-123".to_owned()),
        since_seq: Some(7),
        limit: Some(MAX_NETWORK_REQUEST_LIMIT),
        request_id: Some("ws-1".to_owned()),
        url_contains: Some("socket".to_owned()),
        window_hwnd: None,
    })
    .expect("valid websocket params pass");
    assert_eq!(ok.since_seq, Some(7));
    assert_eq!(ok.limit, MAX_NETWORK_REQUEST_LIMIT);
    assert_eq!(ok.request_id.as_deref(), Some("ws-1"));
    assert_eq!(ok.url_contains.as_deref(), Some("socket"));

    for error in [
        validate_browser_network_websockets_params(&BrowserNetworkWebSocketsParams {
            limit: Some(0),
            ..Default::default()
        })
        .expect_err("zero limit must be rejected"),
        validate_browser_network_websockets_params(&BrowserNetworkWebSocketsParams {
            request_id: Some(" ws-1".to_owned()),
            ..Default::default()
        })
        .expect_err("request id whitespace must be rejected"),
        validate_browser_network_websockets_params(&BrowserNetworkWebSocketsParams {
            url_contains: Some(String::new()),
            ..Default::default()
        })
        .expect_err("empty url_contains must be rejected"),
    ] {
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID));
    }
    println!("readback=browser_network_websockets validation edges rejected invalid params");
}

#[test]
fn must_fail_browser_network_har_validation_edges() {
    let record = validate_browser_network_har_params(&BrowserNetworkHarParams {
        cdp_target_id: Some("target-123".to_owned()),
        operation: BrowserNetworkHarOperation::Record,
        path: Some(r"C:\tmp\capture.har".to_owned()),
        since_seq: Some(3),
        limit: Some(50),
        url_contains: Some("api".to_owned()),
        include_bodies: Some(false),
        ..Default::default()
    })
    .expect("valid HAR record params pass");
    assert_eq!(record.operation, BrowserNetworkHarOperation::Record);
    assert_eq!(record.filters.since_seq, Some(3));
    assert_eq!(record.filters.limit, 50);
    assert!(!record.include_bodies);

    let replay = validate_browser_network_har_params(&BrowserNetworkHarParams {
        operation: BrowserNetworkHarOperation::Replay,
        path: Some(r"C:\tmp\capture.har".to_owned()),
        limit: Some(25),
        missing_policy: Some(BrowserNetworkHarMissingPolicy::Abort),
        clear_existing_replay: Some(false),
        ..Default::default()
    })
    .expect("valid HAR replay params pass");
    assert_eq!(replay.operation, BrowserNetworkHarOperation::Replay);
    assert_eq!(replay.filters.limit, 25);
    assert_eq!(replay.missing_policy, BrowserNetworkHarMissingPolicy::Abort);
    assert!(!replay.clear_existing_replay);

    let clear = validate_browser_network_har_params(&BrowserNetworkHarParams {
        operation: BrowserNetworkHarOperation::ClearReplay,
        ..Default::default()
    })
    .expect("valid HAR clear params pass");
    assert_eq!(clear.operation, BrowserNetworkHarOperation::ClearReplay);

    for error in [
        validate_browser_network_har_params(&BrowserNetworkHarParams {
            operation: BrowserNetworkHarOperation::Record,
            ..Default::default()
        })
        .expect_err("record requires path"),
        validate_browser_network_har_params(&BrowserNetworkHarParams {
            operation: BrowserNetworkHarOperation::Replay,
            path: Some("capture.har".to_owned()),
            url_contains: Some("api".to_owned()),
            ..Default::default()
        })
        .expect_err("replay rejects record filters"),
        validate_browser_network_har_params(&BrowserNetworkHarParams {
            operation: BrowserNetworkHarOperation::ClearReplay,
            path: Some("capture.har".to_owned()),
            ..Default::default()
        })
        .expect_err("clear rejects path"),
        validate_browser_network_har_params(&BrowserNetworkHarParams {
            operation: BrowserNetworkHarOperation::Record,
            path: Some("capture.har".to_owned()),
            url_contains: Some("api".to_owned()),
            url_regex: Some("api".to_owned()),
            ..Default::default()
        })
        .expect_err("record rejects ambiguous URL filters"),
    ] {
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID));
    }
    println!("readback=browser_network_har validation edges rejected invalid params");
}

#[tokio::test]
async fn must_pass_browser_network_har_record_writes_har_and_replay_rules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("capture.har");
    let entries = vec![entry(
        1,
        "req-1",
        "https://example.test/api/data?x=1",
        "Fetch",
        Some(200),
    )];

    let written = write_har_record("target-1", &path, &entries, false, false)
        .await
        .expect("write HAR");
    assert_eq!(written.entry_count, 1);
    assert_eq!(written.skipped_count, 0);
    assert!(written.bytes_written > 0);
    let raw = fs::read_to_string(&path).expect("read HAR");
    assert!(raw.contains("\"version\": \"1.2\""));
    assert!(raw.contains("\"_synapseRequestId\": \"req-1\""));

    let replay =
        load_har_replay(&path, 10, BrowserNetworkHarMissingPolicy::Abort).expect("load HAR replay");
    assert_eq!(replay.entry_count, 1);
    assert_eq!(replay.skipped_count, 0);
    assert!(replay.missing_abort_route_installed);
    assert_eq!(replay.rules.len(), 2);
    assert_eq!(replay.rules[0].method.as_deref(), Some("GET"));
    assert!(replay.rules[0].url.starts_with('^'));
    assert_eq!(replay.rules[1].id, HAR_REPLAY_MISS_ROUTE_ID);
    println!(
        "readback=browser_network_har record_entries={} replay_rules={}",
        written.entry_count,
        replay.rules.len()
    );
}

#[test]
fn must_pass_browser_network_har_replay_encodes_text_content() {
    let har_entry = HarEntry {
        request: HarRequest {
            method: "POST".to_owned(),
            url: "https://example.test/submit".to_owned(),
            ..Default::default()
        },
        response: HarResponse {
            status: 201,
            status_text: "Created".to_owned(),
            headers: vec![
                HarHeader {
                    name: "content-type".to_owned(),
                    value: "text/plain".to_owned(),
                },
                HarHeader {
                    name: "content-length".to_owned(),
                    value: "999".to_owned(),
                },
            ],
            content: HarContent {
                size: 5,
                mime_type: "text/plain".to_owned(),
                text: Some("hello".to_owned()),
                encoding: None,
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let rule = har_entry_to_route_rule(0, &har_entry)
        .expect("HAR route")
        .expect("route exists");
    assert_eq!(rule.method.as_deref(), Some("POST"));
    match rule.action {
        synapse_a11y::CdpFetchRouteAction::Fulfill(fulfill) => {
            assert_eq!(fulfill.status, 201);
            assert_eq!(fulfill.body_base64.as_deref(), Some("aGVsbG8="));
            assert_eq!(fulfill.headers.len(), 1);
            assert_eq!(fulfill.headers[0].0, "content-type");
        }
        _ => panic!("expected fulfill rule"),
    }
    println!("readback=browser_network_har replay text content encoded as base64");
}

#[test]
fn must_pass_browser_network_websocket_entry_maps_frames_and_close_info() {
    let wire = browser_network_websocket_entry_to_wire(&websocket_entry());
    assert_eq!(wire.request_id, "ws-1");
    assert_eq!(wire.status, Some(101));
    assert_eq!(wire.sent_frame_count, 1);
    assert_eq!(wire.received_frame_count, 1);
    assert!(wire.closed);
    assert_eq!(wire.close_code, Some(1000));
    assert_eq!(wire.close_reason.as_deref(), Some("OK"));
    assert_eq!(wire.frames.len(), 2);
    assert_eq!(wire.frames[0].direction, "sent");
    assert_eq!(wire.frames[0].payload_data.as_deref(), Some("ping"));
    assert_eq!(wire.frames[1].close_code, Some(1000));
    println!(
        "readback=browser_network_websocket wire request_id={} frames={} close_code={:?}",
        wire.request_id,
        wire.frames.len(),
        wire.close_code
    );
}

#[test]
fn must_pass_browser_network_requests_filters_entries_after_cursor_read() {
    let filters = validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
        url_contains: Some("/api/".to_owned()),
        resource_type: Some("XHR".to_owned()),
        status_min: Some(200),
        status_max: Some(299),
        ..Default::default()
    })
    .expect("filters validate");
    let filtered = filter_network_entries(
        vec![
            entry(1, "doc", "https://example.test/", "Document", Some(200)),
            entry(
                2,
                "api-ok",
                "https://example.test/api/users",
                "XHR",
                Some(204),
            ),
            entry(
                3,
                "api-err",
                "https://example.test/api/fail",
                "XHR",
                Some(500),
            ),
        ]
        .into_iter(),
        &filters,
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].request_id, "api-ok");
    let wire = browser_network_entry_to_wire(&filtered[0]);
    assert_eq!(wire.status, Some(204));
    assert_eq!(
        wire.response_headers,
        Some(json!({"content-type": "application/json"}))
    );
    assert_eq!(wire.encoded_data_length, Some(84.0));
}

#[test]
fn must_fail_browser_network_request_validation_edges() {
    let ok = validate_browser_network_request_params(&BrowserNetworkRequestParams {
        request_id: "1234.56".to_owned(),
        cdp_target_id: Some("target-123".to_owned()),
        window_hwnd: Some(100),
        include_body: true,
        include_post_data: true,
    })
    .expect("valid request params pass");
    assert_eq!(ok.request_id, "1234.56");
    assert!(ok.include_body);
    assert!(ok.include_post_data);

    for error in [
        validate_browser_network_request_params(&BrowserNetworkRequestParams {
            request_id: String::new(),
            cdp_target_id: None,
            window_hwnd: None,
            include_body: true,
            include_post_data: true,
        })
        .expect_err("empty request id must be rejected"),
        validate_browser_network_request_params(&BrowserNetworkRequestParams {
            request_id: " request ".to_owned(),
            cdp_target_id: None,
            window_hwnd: None,
            include_body: true,
            include_post_data: true,
        })
        .expect_err("request id whitespace must be rejected"),
        validate_browser_network_request_params(&BrowserNetworkRequestParams {
            request_id: "bad\nid".to_owned(),
            cdp_target_id: None,
            window_hwnd: None,
            include_body: true,
            include_post_data: true,
        })
        .expect_err("request id control chars must be rejected"),
    ] {
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID));
    }
}

#[test]
fn must_pass_browser_network_request_detail_maps_full_entry_and_body_metadata() {
    let mut captured = entry(
        9,
        "api-ok",
        "https://example.test/api/users",
        "XHR",
        Some(200),
    );
    captured.first_seq = 7;
    captured.request_has_post_data = Some(true);
    captured.initiator = Some(json!({"type": "script"}));
    captured
        .redirects
        .push(synapse_a11y::CdpNetworkResponseSnapshot {
            url: "https://example.test/old".to_owned(),
            status: 302,
            status_text: "Found".to_owned(),
            headers: json!({"location": "/api/users"}),
            request_headers: None,
            mime_type: "text/html".to_owned(),
            protocol: Some("h2".to_owned()),
            remote_ip_address: Some("127.0.0.1".to_owned()),
            remote_port: Some(443),
            encoded_data_length: 10.0,
            timing: None,
            response_time_ms: Some(3.0),
            from_disk_cache: Some(false),
            from_service_worker: Some(false),
            from_prefetch_cache: Some(false),
            from_early_hints: Some(false),
            timestamp_s: Some(1.5),
            resource_type: Some("XHR".to_owned()),
        });

    let detail = browser_network_request_detail_to_wire(&captured);
    assert_eq!(detail.seq, 9);
    assert_eq!(detail.first_seq, 7);
    assert_eq!(detail.request_id, "api-ok");
    assert_eq!(detail.request_has_post_data, Some(true));
    assert_eq!(detail.initiator, Some(json!({"type": "script"})));
    assert_eq!(detail.redirects.len(), 1);
    assert_eq!(detail.response.as_ref().map(|r| r.status), Some(200));

    let body = browser_network_response_body_to_wire(synapse_a11y::CdpNetworkResponseBody {
        request_id: "api-ok".to_owned(),
        body: "{\"ok\":true}".to_owned(),
        base64_encoded: false,
    });
    assert_eq!(body.body_len_chars, 11);
    assert!(!body.base64_encoded);

    let post_data = browser_network_post_data_to_wire(synapse_a11y::CdpNetworkRequestPostData {
        request_id: "api-ok".to_owned(),
        post_data: "{\"name\":\"Ada\"}".to_owned(),
    });
    assert_eq!(post_data.post_data_len_chars, 14);
}

#[test]
fn must_fail_browser_network_request_body_requires_completed_response() {
    let mut pending = entry(1, "pending", "https://example.test/api", "XHR", Some(200));
    pending.loading_finished = false;
    let error = require_response_body_available(&pending)
        .expect_err("pending response body must be rejected");
    let code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(code, Some(error_codes::A11Y_CDP_AXTREE_FAILED));
}
