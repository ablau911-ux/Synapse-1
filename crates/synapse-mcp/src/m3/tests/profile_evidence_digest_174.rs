//! Candidate-id compatibility does not exempt patch/summary content from #174.
use super::*;

#[test]
fn must_pass_profile_evidence_digest_keeps_install_local_equality_174() {
    let patch = serde_json::json!({"prompt": "operator-private-instruction"});
    let summary = serde_json::json!({"title": "private-document-title"});
    let digest = evidence_hash("profile-g7b", &[], &[], &patch, &summary).expect("valid evidence");
    assert_eq!(digest.get(..12), Some("hmac-sha256:"));
    assert_eq!(digest.len(), 76);
    assert_eq!(
        evidence_hash("profile-g7b", &[], &[], &patch, &summary).expect("same evidence"),
        digest
    );
}

#[test]
fn must_fail_content_changes_cannot_reuse_candidate_evidence_identity_174() {
    let first = evidence_hash(
        "profile-g7b",
        &[],
        &[],
        &serde_json::json!({"prompt": "private-one"}),
        &serde_json::json!({}),
    )
    .expect("first patch");
    let second = evidence_hash(
        "profile-g7b",
        &[],
        &[],
        &serde_json::json!({"prompt": "private-two"}),
        &serde_json::json!({}),
    )
    .expect("second patch");
    assert_eq!(first.get(..12), Some("hmac-sha256:"));
    assert_eq!(second.get(..12), Some("hmac-sha256:"));
    assert_ne!(
        first, second,
        "different private patch cannot alias candidate identity"
    );
}
