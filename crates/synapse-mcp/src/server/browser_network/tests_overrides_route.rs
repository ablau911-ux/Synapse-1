//! Tests for network overrides and `browser_route`.

use super::types_har_overrides::{BrowserNetworkOverridesOperation, BrowserNetworkOverridesParams};
use super::types_route::{
    BrowserRouteErrorReason, BrowserRouteHeader, BrowserRouteMatchKind, BrowserRouteOperation,
    BrowserRouteParams,
};
use super::validation_overrides::validate_browser_network_overrides_params;
use super::validation_route::validate_browser_route_params;
use super::wire::{
    browser_network_overrides_response, browser_route_fetch_status_from_a11y,
    browser_route_rule_to_wire,
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use synapse_core::error_codes;

#[test]
fn must_fail_browser_network_overrides_validation_edges() {
    let ok = validate_browser_network_overrides_params(&BrowserNetworkOverridesParams {
        cdp_target_id: Some("target-123".to_owned()),
        headers: vec![BrowserRouteHeader {
            name: "x-synapse-test".to_owned(),
            value: "enabled".to_owned(),
        }],
        user_agent: Some("SynapseTest/1.0".to_owned()),
        ..Default::default()
    })
    .expect("valid override params pass");
    assert_eq!(ok.operation, BrowserNetworkOverridesOperation::Set);
    assert_eq!(ok.headers[0].0, "x-synapse-test");
    assert_eq!(ok.user_agent.as_deref(), Some("SynapseTest/1.0"));

    for error in [
        validate_browser_network_overrides_params(&BrowserNetworkOverridesParams {
            headers: vec![BrowserRouteHeader {
                name: "bad header".to_owned(),
                value: "value".to_owned(),
            }],
            ..Default::default()
        })
        .expect_err("bad header name must be rejected"),
        validate_browser_network_overrides_params(&BrowserNetworkOverridesParams {
            user_agent: Some(" bad ".to_owned()),
            ..Default::default()
        })
        .expect_err("bad user-agent whitespace must be rejected"),
        validate_browser_network_overrides_params(&BrowserNetworkOverridesParams {
            operation: BrowserNetworkOverridesOperation::Get,
            user_agent: Some("SynapseTest/1.0".to_owned()),
            ..Default::default()
        })
        .expect_err("get rejects set-only fields"),
        validate_browser_network_overrides_params(&BrowserNetworkOverridesParams {
            operation: BrowserNetworkOverridesOperation::Clear,
            headers: vec![BrowserRouteHeader {
                name: "x-test".to_owned(),
                value: "yes".to_owned(),
            }],
            ..Default::default()
        })
        .expect_err("clear rejects set-only fields"),
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
fn must_pass_browser_network_overrides_response_maps_active_and_inactive_state() {
    let active = browser_network_overrides_response(
        "session-1",
        100,
        "http://127.0.0.1:9222".to_owned(),
        "target-123",
        BrowserNetworkOverridesOperation::Set,
        Some(synapse_a11y::CdpNetworkOverrideStatus {
            newly_armed: true,
            endpoint: "http://127.0.0.1:9222".to_owned(),
            cdp_target_id: "target-123".to_owned(),
            armed_at_unix_ms: 10,
            applied_at_unix_ms: 20,
            header_count: 1,
            headers: vec![("x-synapse-test".to_owned(), "enabled".to_owned())],
            user_agent: Some("SynapseTest/1.0".to_owned()),
            original_user_agent: Some("Chrome/Default".to_owned()),
        }),
        false,
    );
    assert!(active.override_active);
    assert!(active.newly_armed);
    assert_eq!(active.header_count, 1);
    assert_eq!(active.headers[0].name, "x-synapse-test");
    assert_eq!(active.user_agent.as_deref(), Some("SynapseTest/1.0"));
    assert_eq!(
        active.original_user_agent.as_deref(),
        Some("Chrome/Default")
    );

    let inactive = browser_network_overrides_response(
        "session-1",
        100,
        "http://127.0.0.1:9222".to_owned(),
        "target-123",
        BrowserNetworkOverridesOperation::Get,
        None,
        false,
    );
    assert!(!inactive.override_active);
    assert_eq!(inactive.header_count, 0);
    assert!(inactive.headers.is_empty());
}

#[test]
fn must_pass_browser_route_add_fulfill_validation_defaults_and_encodes_body() {
    let normalized = validate_browser_route_params(&BrowserRouteParams {
        cdp_target_id: Some("target-123".to_owned()),
        route_id: Some("api-users".to_owned()),
        url: Some("https://example.test/api/*".to_owned()),
        resource_type: Some("XHR".to_owned()),
        headers: vec![BrowserRouteHeader {
            name: "content-type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body: Some("{\"ok\":true}".to_owned()),
        ..Default::default()
    })
    .expect("valid route params pass");

    assert_eq!(normalized.operation, BrowserRouteOperation::AddFulfill);
    assert_eq!(normalized.route_id.as_deref(), Some("api-users"));
    let route = normalized.route.expect("route normalized");
    assert_eq!(route.id, "api-users");
    assert_eq!(route.url, "https://example.test/api/*");
    assert_eq!(route.match_kind, synapse_a11y::CdpFetchRouteMatchKind::Glob);
    assert_eq!(route.resource_type.as_deref(), Some("XHR"));
    let fulfill = match route.action {
        synapse_a11y::CdpFetchRouteAction::Fulfill(fulfill) => fulfill,
        synapse_a11y::CdpFetchRouteAction::Abort(_) => panic!("expected fulfill rule"),
        synapse_a11y::CdpFetchRouteAction::Continue(_) => panic!("expected fulfill rule"),
    };
    assert_eq!(fulfill.status, 200);
    assert_eq!(fulfill.headers[0].0, "content-type");
    assert_eq!(
        fulfill.body_base64.as_deref(),
        Some(BASE64_STANDARD.encode("{\"ok\":true}").as_str())
    );
}

#[test]
fn must_pass_browser_route_add_abort_validation_defaults_to_blocked_by_client() {
    let normalized = validate_browser_route_params(&BrowserRouteParams {
        operation: BrowserRouteOperation::AddAbort,
        route_id: Some("block-images".to_owned()),
        url: Some("https://example.test/assets/*".to_owned()),
        resource_type: Some("Image".to_owned()),
        ..Default::default()
    })
    .expect("valid abort params pass");

    assert_eq!(normalized.operation, BrowserRouteOperation::AddAbort);
    assert_eq!(normalized.route_id.as_deref(), Some("block-images"));
    let route = normalized.route.expect("route normalized");
    assert_eq!(route.id, "block-images");
    assert_eq!(route.resource_type.as_deref(), Some("Image"));
    let abort = match route.action {
        synapse_a11y::CdpFetchRouteAction::Abort(abort) => abort,
        synapse_a11y::CdpFetchRouteAction::Fulfill(_) => panic!("expected abort rule"),
        synapse_a11y::CdpFetchRouteAction::Continue(_) => panic!("expected abort rule"),
    };
    assert_eq!(abort.error_reason, "BlockedByClient");
}

#[test]
fn must_pass_browser_route_add_continue_validation_encodes_post_data() {
    let normalized = validate_browser_route_params(&BrowserRouteParams {
        operation: BrowserRouteOperation::AddContinue,
        route_id: Some("rewrite-api".to_owned()),
        url: Some("https://example.test/api/*".to_owned()),
        continue_url: Some("https://example.test/mock".to_owned()),
        continue_method: Some("POST".to_owned()),
        continue_headers: vec![BrowserRouteHeader {
            name: "x-test".to_owned(),
            value: "yes".to_owned(),
        }],
        continue_post_data: Some("{\"patched\":true}".to_owned()),
        ..Default::default()
    })
    .expect("valid continue params pass");

    assert_eq!(normalized.operation, BrowserRouteOperation::AddContinue);
    assert_eq!(normalized.route_id.as_deref(), Some("rewrite-api"));
    let route = normalized.route.expect("route normalized");
    let continue_rule = match route.action {
        synapse_a11y::CdpFetchRouteAction::Continue(continue_rule) => continue_rule,
        synapse_a11y::CdpFetchRouteAction::Fulfill(_) => panic!("expected continue rule"),
        synapse_a11y::CdpFetchRouteAction::Abort(_) => panic!("expected continue rule"),
    };
    assert_eq!(
        continue_rule.url.as_deref(),
        Some("https://example.test/mock")
    );
    assert_eq!(continue_rule.method.as_deref(), Some("POST"));
    assert_eq!(continue_rule.headers[0].0, "x-test");
    assert_eq!(
        continue_rule.post_data_base64.as_deref(),
        Some(BASE64_STANDARD.encode("{\"patched\":true}").as_str())
    );
}

#[test]
fn must_fail_browser_route_validation_edges() {
    for error in [
        validate_browser_route_params(&BrowserRouteParams {
            operation: BrowserRouteOperation::Remove,
            ..Default::default()
        })
        .expect_err("remove requires route_id"),
        validate_browser_route_params(&BrowserRouteParams {
            route_id: Some("bad id".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            ..Default::default()
        })
        .expect_err("route id whitespace must be rejected"),
        validate_browser_route_params(&BrowserRouteParams {
            route_id: Some("bad-status".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            status: Some(99),
            ..Default::default()
        })
        .expect_err("bad status must be rejected"),
        validate_browser_route_params(&BrowserRouteParams {
            route_id: Some("bad-regex".to_owned()),
            url: Some("[".to_owned()),
            match_kind: BrowserRouteMatchKind::Regex,
            ..Default::default()
        })
        .expect_err("bad regex must be rejected"),
        validate_browser_route_params(&BrowserRouteParams {
            route_id: Some("two-bodies".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            body: Some("plain".to_owned()),
            body_base64: Some("cGxhaW4=".to_owned()),
            ..Default::default()
        })
        .expect_err("body and body_base64 are mutually exclusive"),
        validate_browser_route_params(&BrowserRouteParams {
            route_id: Some("bad-base64".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            body_base64: Some("not base64".to_owned()),
            ..Default::default()
        })
        .expect_err("invalid base64 must be rejected"),
        validate_browser_route_params(&BrowserRouteParams {
            route_id: Some("bad-header".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            headers: vec![BrowserRouteHeader {
                name: "bad header".to_owned(),
                value: "value".to_owned(),
            }],
            ..Default::default()
        })
        .expect_err("bad header name must be rejected"),
        validate_browser_route_params(&BrowserRouteParams {
            operation: BrowserRouteOperation::AddAbort,
            route_id: Some("abort-with-body".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            body: Some("plain".to_owned()),
            ..Default::default()
        })
        .expect_err("abort rejects fulfill-only fields"),
        validate_browser_route_params(&BrowserRouteParams {
            route_id: Some("fulfill-with-reason".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            error_reason: Some(BrowserRouteErrorReason::Aborted),
            ..Default::default()
        })
        .expect_err("fulfill rejects error_reason"),
        validate_browser_route_params(&BrowserRouteParams {
            operation: BrowserRouteOperation::AddContinue,
            route_id: Some("continue-empty".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            ..Default::default()
        })
        .expect_err("continue requires at least one override"),
        validate_browser_route_params(&BrowserRouteParams {
            operation: BrowserRouteOperation::AddContinue,
            route_id: Some("continue-bad-method".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            continue_method: Some("BAD METHOD".to_owned()),
            ..Default::default()
        })
        .expect_err("continue method token must be valid"),
        validate_browser_route_params(&BrowserRouteParams {
            operation: BrowserRouteOperation::AddContinue,
            route_id: Some("continue-two-bodies".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            continue_post_data: Some("plain".to_owned()),
            continue_post_data_base64: Some("cGxhaW4=".to_owned()),
            ..Default::default()
        })
        .expect_err("continue postData inputs are mutually exclusive"),
        validate_browser_route_params(&BrowserRouteParams {
            operation: BrowserRouteOperation::AddContinue,
            route_id: Some("continue-fulfill-fields".to_owned()),
            url: Some("https://example.test/*".to_owned()),
            status: Some(204),
            continue_method: Some("GET".to_owned()),
            ..Default::default()
        })
        .expect_err("continue rejects fulfill-only fields"),
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
fn must_pass_browser_route_rule_wire_hides_body_content() {
    let rule = synapse_a11y::CdpFetchRouteRule {
        id: "api-users".to_owned(),
        url: "https://example.test/api/*".to_owned(),
        match_kind: synapse_a11y::CdpFetchRouteMatchKind::Regex,
        method: None,
        resource_type: Some("XHR".to_owned()),
        action: synapse_a11y::CdpFetchRouteAction::Fulfill(synapse_a11y::CdpFetchRouteFulfill {
            status: 204,
            response_phrase: Some("No Content".to_owned()),
            headers: vec![("x-test".to_owned(), "yes".to_owned())],
            body_base64: Some("c2VjcmV0".to_owned()),
        }),
    };

    let wire = browser_route_rule_to_wire(&rule);
    assert_eq!(wire.id, "api-users");
    assert_eq!(wire.match_kind, BrowserRouteMatchKind::Regex);
    assert_eq!(wire.status, Some(204));
    assert_eq!(wire.error_reason, None);
    assert_eq!(wire.response_phrase.as_deref(), Some("No Content"));
    assert_eq!(wire.headers[0].name, "x-test");
    assert_eq!(wire.body_base64_len_chars, Some(8));
}

#[test]
fn must_pass_browser_route_rule_wire_maps_abort_reason() {
    let rule = synapse_a11y::CdpFetchRouteRule {
        id: "block-images".to_owned(),
        url: "https://example.test/assets/*".to_owned(),
        match_kind: synapse_a11y::CdpFetchRouteMatchKind::Glob,
        method: None,
        resource_type: Some("Image".to_owned()),
        action: synapse_a11y::CdpFetchRouteAction::Abort(synapse_a11y::CdpFetchRouteAbort {
            error_reason: "BlockedByClient".to_owned(),
        }),
    };

    let wire = browser_route_rule_to_wire(&rule);
    assert_eq!(wire.id, "block-images");
    assert_eq!(wire.action, "abort");
    assert_eq!(wire.status, None);
    assert_eq!(wire.error_reason.as_deref(), Some("BlockedByClient"));
    assert!(wire.headers.is_empty());
    assert_eq!(wire.body_base64_len_chars, None);
    assert_eq!(wire.post_data_base64_len_chars, None);
}

#[test]
fn must_pass_browser_route_rule_wire_maps_continue_overrides() {
    let rule = synapse_a11y::CdpFetchRouteRule {
        id: "rewrite-api".to_owned(),
        url: "https://example.test/api/*".to_owned(),
        match_kind: synapse_a11y::CdpFetchRouteMatchKind::Glob,
        method: None,
        resource_type: Some("Fetch".to_owned()),
        action: synapse_a11y::CdpFetchRouteAction::Continue(synapse_a11y::CdpFetchRouteContinue {
            url: Some("https://example.test/mock".to_owned()),
            method: Some("POST".to_owned()),
            headers: vec![("x-test".to_owned(), "yes".to_owned())],
            post_data_base64: Some("eyJwYXRjaGVkIjp0cnVlfQ==".to_owned()),
        }),
    };

    let wire = browser_route_rule_to_wire(&rule);
    assert_eq!(wire.id, "rewrite-api");
    assert_eq!(wire.action, "continue");
    assert_eq!(wire.status, None);
    // Public route readbacks redact URL paths/queries so rewrite targets cannot
    // leak secrets to the client (see url_redaction + commit 93983a2d). The
    // origin is preserved but the `/mock` path is replaced with `/redacted`.
    assert_eq!(
        wire.continue_url.as_deref(),
        Some("https://example.test/redacted")
    );
    assert!(
        !wire.continue_url.as_deref().unwrap().contains("mock"),
        "continue_url path must be redacted, got: {:?}",
        wire.continue_url
    );
    assert_eq!(wire.continue_method.as_deref(), Some("POST"));
    assert_eq!(wire.headers[0].name, "x-test");
    assert_eq!(wire.post_data_base64_len_chars, Some(24));
}

#[test]
fn must_pass_browser_route_fetch_status_maps_a11y_counters() {
    let wire = browser_route_fetch_status_from_a11y(
        Some(synapse_a11y::CdpFetchInterceptionStatus {
            newly_armed: true,
            endpoint: "http://127.0.0.1:9222".to_owned(),
            cdp_target_id: "target-123".to_owned(),
            armed_at_unix_ms: 42,
            pattern_count: 0,
            route_count: 2,
            paused_count: 3,
            continued_count: 1,
            fulfilled_count: 2,
            failed_count: 1,
            continue_error_count: 0,
            last_request_id: Some("fetch-1".to_owned()),
            last_url: Some("https://example.test/api".to_owned()),
            last_route_id: Some("api-users".to_owned()),
            last_error: None,
        }),
        true,
    );

    assert!(wire.fetch_armed);
    assert!(wire.newly_armed);
    assert_eq!(wire.route_count, 2);
    assert_eq!(wire.fulfilled_count, 2);
    assert_eq!(wire.failed_count, 1);
    assert_eq!(wire.last_route_id.as_deref(), Some("api-users"));
}
