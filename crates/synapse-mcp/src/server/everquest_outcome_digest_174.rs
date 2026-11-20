//! Regression tests for #174: the EverQuest outcome row identity (log-line
//! digest and the row-key suffix derived from it) is keyed, while the pre-#174
//! plain identity stays computable to find rows persisted before the change.

use super::*;

const LINE: &[u8] = b"[Thu May 28 11:00:00 2026] You gain experience!!";

#[test]
fn must_pass_line_identity_is_keyed_and_not_a_plain_sha256() {
    let keyed = line_identity(42, LINE);
    let plain = sha256_hex(LINE);
    let keyed_hex = crate::content_digest::keyed_digest_hex(LINE);

    assert_eq!(
        keyed.content_sha256,
        format!("{}{keyed_hex}", crate::content_digest::KEYED_DIGEST_PREFIX)
    );
    assert_eq!(
        keyed.event_id,
        format!("{:016x}-{}", 42_u64, &keyed_hex[..16])
    );
    assert!(!keyed.content_sha256.contains(&plain));
    assert!(
        !keyed.event_id.contains(&plain[..16]),
        "id suffix must not leak the plain digest"
    );
    assert_eq!(
        keyed,
        line_identity(42, LINE),
        "same line, same identity in one install"
    );
    assert_ne!(keyed, line_identity(42, b"another line"));
}

#[test]
fn must_pass_event_id_has_offset_prefix_and_sixteen_hex_suffix() {
    let identity = line_identity(42, LINE);
    let (offset, suffix) = identity.event_id.split_once('-').unwrap_or(("", ""));
    assert_eq!(offset, format!("{:016x}", 42_u64));
    assert_eq!(suffix.len(), 16);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn must_pass_legacy_identity_matches_pre_174_rows() {
    let legacy = legacy_line_identity(42, LINE);
    let plain = sha256_hex(LINE);
    assert_eq!(legacy.content_sha256, plain);
    assert_eq!(legacy.event_id, format!("{:016x}-{}", 42_u64, &plain[..16]));
    assert_ne!(legacy, line_identity(42, LINE));
}

/// The real ingest path: a row persisted before #174 under the plain-digest
/// key is found through the legacy identity, so the same log line is updated in
/// place and never stored a second time under the keyed identity.
#[test]
fn must_pass_ingest_dedupes_against_legacy_row() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let service = SynapseService::try_new_isolated_for_tests(temp.path()).expect("service");
    let legacy = legacy_line_identity(0, LINE);
    let keyed = line_identity(0, LINE);
    let legacy_key = outcome_row_key(EVERQUEST_PROFILE_ID, &legacy.event_id);
    let keyed_key = outcome_row_key(EVERQUEST_PROFILE_ID, &keyed.event_id);
    {
        let runtime = service.reflex_runtime().expect("runtime");
        let runtime = runtime.lock().expect("runtime lock");
        runtime
            .storage_put_kv_rows(vec![(legacy_key.as_bytes().to_vec(), b"{}".to_vec())])
            .expect("plant pre-#174 row");
    }
    let source = EverQuestOutcomeIngestSource {
        path: "C:/synthetic/eqlog_Test_server.txt".to_owned(),
        start_offset: 0,
        next_offset: u64::try_from(LINE.len() + 1).expect("len"),
        file_len_bytes: u64::try_from(LINE.len() + 1).expect("len"),
        bytes_read: LINE.len() + 1,
        truncated_by_bytes: false,
        truncated_by_events: false,
        explicit_log_path: true,
        active_character: None,
        server: "server".to_owned(),
    };
    let lines = vec![ParsedOutcomeLine {
        line_index_in_read: 0,
        start_offset: 0,
        next_offset: u64::try_from(LINE.len() + 1).expect("len"),
        content: LINE.to_vec(),
    }];

    let response = service
        .persist_outcome_rows(EVERQUEST_PROFILE_ID, source, lines, true)
        .expect("ingest succeeds");

    assert_eq!(response.rows_persisted, 1);
    assert_eq!(
        response.duplicate_rows, 1,
        "the legacy row is the same line"
    );
    let row = &response.rows[0];
    assert_eq!(row.row_key, legacy_key);
    assert_eq!(row.event_id, legacy.event_id);
    assert_eq!(row.source.content_sha256, legacy.content_sha256);
    assert!(row.duplicate_of_prior_row);
    let runtime = service.reflex_runtime().expect("runtime");
    let runtime = runtime.lock().expect("runtime lock");
    assert!(
        runtime
            .storage_kv_row(keyed_key.as_bytes())
            .expect("read keyed key")
            .is_none(),
        "no second row under the keyed identity"
    );
}
