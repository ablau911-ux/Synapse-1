//! Regression tests for #174: lifecycle audit row digests (raw line and the
//! MCP session id) are keyed `hmac-sha256:` tokens, not plain SHA-256.

use serde_json::json;
use sha2::{Digest as _, Sha256};

use super::*;

fn plain_sha256(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[test]
fn must_pass_lifecycle_row_digests_are_keyed_and_stable_within_install() {
    let line = br#"{"mcp_session_id":"1234","event_kind":"tool"}"#;
    let value = json!({"mcp_session_id": "1234", "event_kind": "tool"});

    let first = summarize_lifecycle_row(1, line, &value);
    let second = summarize_lifecycle_row(2, line, &value);

    assert!(
        first.raw_sha256.starts_with("hmac-sha256:"),
        "{}",
        first.raw_sha256
    );
    assert_ne!(first.raw_sha256, plain_sha256(line));
    assert_eq!(
        first.raw_sha256, second.raw_sha256,
        "equal lines, equal digest"
    );

    let session = first.mcp_session_id_sha256.as_deref().unwrap_or_default();
    assert!(session.starts_with("hmac-sha256:"), "{session}");
    assert_ne!(
        session,
        plain_sha256(b"1234"),
        "low-entropy id must not be plain-hashed"
    );
    assert_eq!(first.mcp_session_id_sha256, second.mcp_session_id_sha256);
}

#[test]
fn must_fail_lifecycle_row_without_session_id_has_no_session_digest() {
    let value = json!({"event_kind": "tool"});
    let row = summarize_lifecycle_row(1, b"{}", &value);
    assert!(!row.mcp_session_id_present);
    assert_eq!(row.mcp_session_id_sha256, None);
}

#[test]
fn must_pass_whole_artifact_digest_stays_plain_for_sha256sum_comparison() {
    assert_eq!(
        super::super::util::prefixed_sha256(b"abc"),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
