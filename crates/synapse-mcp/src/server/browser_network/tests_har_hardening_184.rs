//! Regression tests for #184: `browser_network_har` must not let page-driven
//! prompts write arbitrary files, overwrite existing files, or persist secrets.

use super::har_io::{har_entry_to_route_rule, write_har_record};
use super::har_types::{HarContent, HarEntry, HarHeader, HarRequest, HarResponse};
use super::tests_support::entry;
use super::types_har_overrides::{BrowserNetworkHarOperation, BrowserNetworkHarParams};
use super::validation_har::validate_browser_network_har_params;
use crate::server::ErrorData;
use serde_json::json;
use std::fs;
use synapse_core::error_codes;

fn params(operation: BrowserNetworkHarOperation, path: &str) -> BrowserNetworkHarParams {
    BrowserNetworkHarParams {
        operation,
        path: Some(path.to_owned()),
        ..Default::default()
    }
}

fn assert_params_invalid(error: &ErrorData, context: &str) {
    let code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID), "{context}");
}

fn assert_path_rejected(path: &str) {
    for operation in [
        BrowserNetworkHarOperation::Record,
        BrowserNetworkHarOperation::Replay,
    ] {
        let error = validate_browser_network_har_params(&params(operation, path))
            .expect_err(&format!("{operation:?} must reject path {path:?}"));
        assert_params_invalid(&error, path);
    }
}

#[test]
fn must_fail_har_path_requires_har_extension_case_insensitive() {
    for path in [
        r"C:\tmp\capture.har",
        r"C:\tmp\CAPTURE.HAR",
        r"C:\tmp\capture.Har",
        r"C:/tmp/capture.har",
        "capture.har",
        r"C:\tmp\console.har",
        r"C:\tmp\com10.har",
        r"C:\tmp\nullable.har",
    ] {
        for operation in [
            BrowserNetworkHarOperation::Record,
            BrowserNetworkHarOperation::Replay,
        ] {
            validate_browser_network_har_params(&params(operation, path))
                .unwrap_or_else(|error| panic!("{path:?} must be accepted: {error:?}"));
        }
    }
    for path in [
        r"C:\tmp\capture",
        r"C:\tmp\capture.json",
        r"C:\tmp\capture.cmd",
        r"C:\tmp\capture.har.cmd",
        r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup\x.bat",
        r"C:\tmp\.har",
        r"C:\tmp\capture.har.",
        r"C:\tmp\bad.\capture.har",
    ] {
        assert_path_rejected(path);
    }
}

#[test]
fn must_fail_har_path_rejects_unc_and_device_paths() {
    for path in [
        r"\\server\share\capture.har",
        "//server/share/capture.har",
        r"\\?\C:\tmp\capture.har",
        r"\\.\C:\tmp\capture.har",
        r"\\.\pipe\capture.har",
        r"\\?\UNC\server\share\capture.har",
        r"\??\C:\tmp\capture.har",
    ] {
        assert_path_rejected(path);
    }
}

#[test]
fn must_fail_har_path_rejects_alternate_data_streams_and_drive_relative() {
    for path in [
        r"C:\tmp\capture.har:stream",
        r"C:\tmp\capture.har:evil.cmd",
        r"C:\tmp\capture.har::$DATA",
        "capture.har:stream",
        r"C:\tmp:stream\capture.har",
        "C:capture.har",
        r"C::\capture.har",
    ] {
        assert_path_rejected(path);
    }
}

#[test]
fn must_fail_har_path_rejects_reserved_device_names() {
    for path in [
        r"C:\tmp\NUL.har",
        r"C:\tmp\con.har",
        r"C:\tmp\Prn.har",
        r"C:\tmp\AUX.har",
        r"C:\tmp\COM1.har",
        r"C:\tmp\lpt9.har",
        r"C:\tmp\nul.txt.har",
        r"C:\tmp\CONIN$.har",
        r"C:\tmp\CONOUT$.har",
        r"C:\tmp\aux\capture.har",
        r"NUL.har",
    ] {
        assert_path_rejected(path);
    }
}

#[test]
fn must_pass_har_record_defaults_exclude_bodies_and_overwrite() {
    let default = validate_browser_network_har_params(&params(
        BrowserNetworkHarOperation::Record,
        r"C:\tmp\capture.har",
    ))
    .expect("valid record params");
    assert!(!default.include_bodies, "include_bodies must default false");
    assert!(!default.overwrite, "overwrite must default false");

    let opted_in = validate_browser_network_har_params(&BrowserNetworkHarParams {
        include_bodies: Some(true),
        overwrite: Some(true),
        ..params(BrowserNetworkHarOperation::Record, r"C:\tmp\capture.har")
    })
    .expect("explicit opt-in params");
    assert!(opted_in.include_bodies);
    assert!(opted_in.overwrite);

    let error = validate_browser_network_har_params(&BrowserNetworkHarParams {
        overwrite: Some(true),
        ..params(BrowserNetworkHarOperation::Replay, r"C:\tmp\capture.har")
    })
    .expect_err("overwrite is only valid for record");
    assert_params_invalid(&error, "replay overwrite");
}

#[tokio::test]
async fn must_fail_har_record_refuses_to_overwrite_existing_file_without_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("capture.har");
    fs::write(&path, b"precious").expect("seed existing file");
    let entries = vec![entry(
        1,
        "req-1",
        "https://example.test/a",
        "Fetch",
        Some(200),
    )];

    let error = write_har_record("target-1", &path, &entries, false, false)
        .await
        .expect_err("existing file must not be overwritten by default");
    assert_params_invalid(&error, "overwrite refusal");
    assert_eq!(fs::read(&path).expect("read"), b"precious");

    let written = write_har_record("target-1", &path, &entries, false, true)
        .await
        .expect("overwrite=true replaces the file");
    assert_eq!(written.entry_count, 1);
    assert!(
        fs::read_to_string(&path)
            .expect("read")
            .contains("\"version\": \"1.2\"")
    );

    let fresh = dir.path().join("fresh.har");
    write_har_record("target-1", &fresh, &entries, false, false)
        .await
        .expect("new file needs no overwrite flag");
    assert!(fresh.exists());
}

#[tokio::test]
async fn must_fail_har_record_redacts_credential_headers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secrets.har");
    let mut captured = entry(1, "req-1", "https://example.test/a", "Fetch", Some(200));
    captured.request_headers = Some(json!({
        "Authorization": "Bearer sekrit-token",
        "proxy-authorization": "Basic proxy-sekrit",
        "COOKIE": "session=sekrit-cookie",
        "accept": "*/*",
    }));
    if let Some(response) = captured.response.as_mut() {
        response.headers = json!({
            "Set-Cookie": "session=sekrit-set-cookie; HttpOnly",
            "content-type": "application/json",
        });
    }

    write_har_record("target-1", &path, &[captured], false, false)
        .await
        .expect("write HAR");
    let raw = fs::read_to_string(&path).expect("read HAR");
    for secret in [
        "sekrit-token",
        "proxy-sekrit",
        "sekrit-cookie",
        "sekrit-set-cookie",
    ] {
        assert!(!raw.contains(secret), "HAR leaked {secret}");
    }
    assert!(raw.contains("[REDACTED]"));
    assert!(raw.contains("Authorization"), "header name is kept");
    assert!(raw.contains("application/json"), "benign headers kept");
    assert!(raw.contains("*/*"), "benign request headers kept");
}

#[test]
fn must_fail_har_replay_skips_redacted_response_headers() {
    let har_entry = HarEntry {
        request: HarRequest {
            method: "GET".to_owned(),
            url: "https://example.test/a".to_owned(),
            ..Default::default()
        },
        response: HarResponse {
            status: 200,
            headers: vec![
                HarHeader {
                    name: "Set-Cookie".to_owned(),
                    value: "[REDACTED]".to_owned(),
                },
                HarHeader {
                    name: "content-type".to_owned(),
                    value: "text/plain".to_owned(),
                },
            ],
            content: HarContent::default(),
            ..Default::default()
        },
        ..Default::default()
    };
    let rule = har_entry_to_route_rule(0, &har_entry)
        .expect("route")
        .expect("exists");
    match rule.action {
        synapse_a11y::CdpFetchRouteAction::Fulfill(fulfill) => {
            assert_eq!(fulfill.headers.len(), 1);
            assert_eq!(fulfill.headers[0].0, "content-type");
        }
        _ => panic!("expected fulfill rule"),
    }
}
