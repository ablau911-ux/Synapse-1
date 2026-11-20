//! Dashboard steer/workspace payloads contain agent-authored content (#174).
use super::*;

#[test]
fn must_pass_dashboard_payload_digest_is_exact_install_keyed_token_174() {
    let payload = serde_json::json!({"message":"g7b-private-message"});
    let encoded = serde_json::to_vec(&payload).expect("fixture");
    let expected = crate::content_digest::keyed_prefixed_digest(&encoded);
    assert_eq!(dashboard_payload_sha256(&payload), expected);
    assert_eq!(
        dashboard_payload_sha256(&payload),
        expected,
        "equality semantics"
    );
}

#[test]
fn must_fail_private_dashboard_messages_never_produce_plain_content_digest_174() {
    let first = dashboard_payload_sha256(&serde_json::json!({"message":"private-one"}));
    let second = dashboard_payload_sha256(&serde_json::json!({"message":"private-two"}));
    assert_eq!(first.get(..12), Some("hmac-sha256:"));
    assert_eq!(second.get(..12), Some("hmac-sha256:"));
    assert_ne!(first, second);
}
