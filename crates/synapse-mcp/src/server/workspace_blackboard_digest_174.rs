//! Regression tests for #174: the workspace row digest readback (also the
//! compare-and-swap binding of `expected_corrupt_sha256`) must be a keyed
//! `hmac-sha256:` token, while a plain `sha256:` digest a client captured
//! before the upgrade must still authorize the delete of that exact row.

use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use super::workspace_blackboard_tests::service_with_db;
use super::*;

const RUN: &str = "run-digest-174";

fn plain_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

fn seed_corrupt(service: &SynapseService, key: &str, value: &[u8]) -> anyhow::Result<String> {
    let row_key = workspace_row_key(RUN, key);
    service
        .workspace_db()?
        .put_batch_pressure_bypass(cf::CF_KV, [(row_key.as_bytes().to_vec(), value.to_vec())])?;
    Ok(row_key)
}

fn delete_with(
    service: &SynapseService,
    key: &str,
    expected: &str,
    now: u64,
) -> Result<WorkspaceDeleteResponse, ErrorData> {
    service.workspace_delete_impl_at(
        WorkspaceDeleteParams {
            run_id: Some(RUN.to_owned()),
            key: key.to_owned(),
            raw_row_key: None,
            expected_version: None,
            expected_corrupt_sha256: Some(expected.to_owned()),
        },
        "session-174",
        now,
    )
}

#[test]
fn must_pass_workspace_row_readback_is_keyed_not_plain_sha256() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let value = b"{not-json 1234".to_vec();
    let row_key = seed_corrupt(&service, "findings/pin", &value)?;

    let readback = readback_exact_workspace_row(&*service.workspace_db()?, &row_key)?;
    assert!(
        readback.value_sha256.starts_with("hmac-sha256:"),
        "{}",
        readback.value_sha256
    );
    assert_ne!(
        readback.value_sha256,
        plain_sha256(&value),
        "digest must not be a brute-forceable plain SHA-256"
    );
    assert_eq!(
        readback.value_sha256,
        crate::content_digest::keyed_prefixed_digest(&value),
        "equal bytes keep equal digests within one install"
    );
    Ok(())
}

#[test]
fn must_fail_corrupt_delete_cas_rejects_a_foreign_digest_and_keeps_the_row() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let value = b"{not-json cas".to_vec();
    let row_key = seed_corrupt(&service, "findings/cas", &value)?;
    let db = service.workspace_db()?;

    // Digest of different bytes (keyed or plain) must not authorize the delete.
    for (n, foreign) in [
        plain_sha256(b"other"),
        crate::content_digest::keyed_prefixed_digest(b"other"),
        "hmac-sha256:".to_owned(),
    ]
    .iter()
    .enumerate()
    {
        let err = delete_with(&service, "findings/cas", foreign, 1_000 + n as u64)
            .err()
            .ok_or_else(|| anyhow::anyhow!("foreign digest must be rejected"))?;
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|d| d.get("code"))
                .and_then(|c| c.as_str()),
            Some("WORKSPACE_CORRUPT_HASH_CONFLICT")
        );
        let kept = readback_exact_workspace_row(&db, &row_key)?;
        assert_eq!(
            kept.value_len_bytes,
            value.len() as u64,
            "row bytes untouched"
        );
    }
    Ok(())
}

#[test]
fn must_pass_corrupt_delete_cas_accepts_legacy_and_keyed_digest_and_removes_the_row()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let db = service.workspace_db()?;

    // A plain digest read before the upgrade still binds to this exact row.
    let value = b"{not-json cas".to_vec();
    let row_key = seed_corrupt(&service, "findings/cas", &value)?;
    let deleted = delete_with(&service, "findings/cas", &plain_sha256(&value), 2_000)?;
    assert_eq!(deleted.deleted_version, None);
    let gone = readback_absent_workspace_row(&db, &row_key)?;
    assert!(!gone.exists, "legacy-digest delete must remove the row");
    assert_eq!(gone.exact_match_count, 0);

    // The keyed digest binds a second row the same way.
    let value2 = b"{not-json cas2".to_vec();
    let row_key2 = seed_corrupt(&service, "findings/cas2", &value2)?;
    let keyed = crate::content_digest::keyed_prefixed_digest(&value2);
    let deleted2 = delete_with(&service, "findings/cas2", &keyed, 3_000)?;
    assert_eq!(deleted2.deleted_version, None);
    let gone2 = readback_absent_workspace_row(&db, &row_key2)?;
    assert!(!gone2.exists, "keyed-digest delete must remove the row");
    assert_eq!(gone2.exact_match_count, 0);
    Ok(())
}
