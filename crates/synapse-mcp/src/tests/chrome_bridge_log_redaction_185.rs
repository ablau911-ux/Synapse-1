//! #185: the persisted `CHROME_DEBUGGER_RESPONSE_ACCEPTED` readback summary
//! must never carry raw page URL query/fragment secrets.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};

use super::chrome_response_readback_summary;

const SECRET_URL: &str =
    "https://login.example.test/cb?code=SYNAPSE_OAUTH_CODE#access_token=SYNAPSE_TOKEN";

/// Public-readback form of [`SECRET_URL`]: path, query and fragment replaced
/// by the `redacted` marker (`url_redaction::redact_parsed_url`).
const REDACTED_URL: &str = "https://login.example.test/redacted?redacted#redacted";

fn assert_clean(kind: &str, result: &Value, url_fields: &[&str]) {
    let summary = chrome_response_readback_summary(kind, Some(result)).unwrap();
    for secret in [
        "SYNAPSE_OAUTH_CODE",
        "SYNAPSE_TOKEN",
        "code=",
        "access_token",
    ] {
        assert!(
            !summary.contains(secret),
            "{kind} leaked {secret}: {summary}"
        );
    }
    let parsed: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(
        parsed.get("tab_id"),
        Some(&json!(1)),
        "{kind} summary lost non-URL fields: {summary}"
    );
    // Redacted, not dropped: every URL field keeps its public-readback form.
    for field in url_fields {
        assert_eq!(
            parsed.get(*field).and_then(Value::as_str),
            Some(REDACTED_URL),
            "{kind}.{field} must carry the redacted URL: {summary}"
        );
    }
}

#[test]
fn must_fail_every_url_bearing_kind_is_redacted() {
    assert_clean(
        "openTab",
        &json!({"tab_id": 1, "url": SECRET_URL}),
        &["url"],
    );
    assert_clean(
        "navigateTab",
        &json!({
            "tab_id": 1,
            "requested_url": SECRET_URL,
            "before_url": SECRET_URL,
            "after_url": SECRET_URL
        }),
        &["requested_url", "before_url", "after_url"],
    );
    assert_clean(
        "pageVitals",
        &json!({"tab_id": 1, "url": SECRET_URL, "title": "t"}),
        &["url"],
    );
    assert_clean(
        "pageContent",
        &json!({"tab_id": 1, "url": SECRET_URL}),
        &["url"],
    );
    assert_clean(
        "setContent",
        &json!({"tab_id": 1, "before_url": SECRET_URL, "after_url": SECRET_URL}),
        &["before_url", "after_url"],
    );
    assert_clean(
        "targetInfo",
        &json!({"tab_id": 1, "url": SECRET_URL}),
        &["url"],
    );
    assert_clean(
        "capturePageScreenshot",
        &json!({"tab_id": 1, "url": SECRET_URL}),
        &["url"],
    );
    assert_clean("clock", &json!({"tab_id": 1, "url": SECRET_URL}), &["url"]);
}

#[test]
fn must_fail_content_bearing_and_unparseable_urls_are_redacted() {
    for url in [
        "data:text/html,SYNAPSE_TOKEN",
        "javascript:SYNAPSE_TOKEN()",
        "not a url?code=SYNAPSE_TOKEN",
    ] {
        let summary =
            chrome_response_readback_summary("openTab", Some(&json!({"tab_id": 1, "url": url})))
                .unwrap();
        assert!(
            !summary.contains("SYNAPSE_TOKEN"),
            "{url} leaked: {summary}"
        );
    }
}

#[test]
fn must_pass_empty_and_absent_urls_still_summarize() {
    let summary =
        chrome_response_readback_summary("openTab", Some(&json!({"tab_id": 1, "url": ""})))
            .unwrap();
    assert!(summary.contains("tab_id"));
    assert!(chrome_response_readback_summary("openTab", None).is_none());
}
