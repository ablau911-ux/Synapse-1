//! #57: bounded maintenance uses the primary row as authority, across reopen.
use super::*;
use std::sync::Arc;

const NOW: u64 = 1_000_000;
const INDEX: &[u8] = b"agent-mailbox-expiry/v1/";

#[test]
fn must_pass_real_send_persists_matching_primary_and_expiry_index_57() {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    use super::mailbox_sweep_throttle_tests::{register, send, service};
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).unwrap();
    for session in ["sender", "r1"] {
        register(&svc, session, NOW).unwrap();
    }
    let db = svc.mailbox_db().unwrap();
    assert_eq!(indexes(&db).len(), 0);
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())
            .unwrap()
            .len(),
        0
    );
    let sent = send(&svc, "r1", 1_000, NOW).unwrap();
    let rows = db
        .scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())
        .unwrap();
    assert_eq!(rows.len(), 1);
    let message: AgentMailboxMessage = synapse_storage::decode_json(&rows[0].1).unwrap();
    assert_eq!(message.message_id, sent.message_id);
    assert_eq!(indexes(&db), vec![expiry::expiry_row(&message).unwrap()]);
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        1
    );
    assert_eq!(indexes(&db).len(), 0);
    assert_eq!(db.get_cf_exact(cf::CF_KV, &rows[0].0).unwrap(), None);
}

fn database(path: &std::path::Path) -> Arc<Db> {
    Arc::new(Db::open(path, SCHEMA_VERSION).unwrap())
}

fn message(seq: u64, ttl: u64) -> AgentMailboxMessage {
    let id = format!("expiry57-{seq:04}");
    AgentMailboxMessage {
        schema_version: SCHEMA_VERSION,
        message_id: id.clone(),
        row_key: mailbox_row_key("expiry57", NOW, seq, &id),
        from_session: "sender".into(),
        to_session: "expiry57".into(),
        kind: "handoff".into(),
        payload: json!({"seq": seq}),
        artifact_handle: None,
        sent_at_unix_ms: NOW,
        ttl_ms: ttl,
        expires_at_unix_ms: NOW + ttl,
        delivery_attempts: 0,
        request_receipt: false,
    }
}

fn primary(db: &Db, row: &AgentMailboxMessage) {
    db.put_batch_pressure_bypass(
        cf::CF_KV,
        [(
            row.row_key.as_bytes().to_vec(),
            synapse_storage::encode_json(row).unwrap(),
        )],
    )
    .unwrap();
    let bytes = db
        .get_cf_exact(cf::CF_KV, row.row_key.as_bytes())
        .unwrap()
        .unwrap();
    let stored: AgentMailboxMessage = synapse_storage::decode_json(&bytes).unwrap();
    assert_eq!(stored.message_id, row.message_id);
    assert_eq!(stored.expires_at_unix_ms, row.expires_at_unix_ms);
}

fn indexes(db: &Db) -> Vec<(Vec<u8>, Vec<u8>)> {
    db.scan_cf_prefix(cf::CF_KV, INDEX).unwrap()
}

#[test]
fn must_pass_indexed_expiry_pages_delete_primary_and_index_together_57() {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    for seq in 0..129 {
        let row = message(seq, 1_000);
        primary(&db, &row);
        db.put_batch_pressure_bypass(cf::CF_KV, [expiry::expiry_row(&row).unwrap()])
            .unwrap();
    }
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())
            .unwrap()
            .len(),
        129
    );
    assert_eq!(indexes(&db).len(), 129);
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        128
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        indexes(&db).len(),
        1,
        "deleted page cannot leave dangling indexes"
    );
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        1
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())
            .unwrap()
            .len(),
        0
    );
    assert_eq!(indexes(&db).len(), 0);
}

#[test]
fn must_pass_legacy_index_build_advances_one_bounded_page_57() {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    for seq in 0..129 {
        primary(&db, &message(seq, 600_000));
    }
    assert_eq!(indexes(&db).len(), 0);
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        0
    );
    assert_eq!(indexes(&db).len(), 128);
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        0
    );
    assert_eq!(indexes(&db).len(), 129);
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())
            .unwrap()
            .len(),
        129
    );
}

#[test]
fn must_fail_stale_due_index_cannot_delete_extended_primary_57() {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    let old = message(1, 1_000);
    let (old_key, old_value) = expiry::expiry_row(&old).unwrap();
    let mut extended = old.clone();
    extended.ttl_ms = 600_000;
    extended.expires_at_unix_ms = NOW + 600_000;
    primary(&db, &extended);
    db.put_batch_pressure_bypass(cf::CF_KV, [(old_key.clone(), old_value)])
        .unwrap();
    assert!(db.get_cf_exact(cf::CF_KV, &old_key).unwrap().is_some());
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        0
    );
    assert!(
        db.get_cf_exact(cf::CF_KV, old.row_key.as_bytes())
            .unwrap()
            .is_some()
    );
    assert_eq!(db.get_cf_exact(cf::CF_KV, &old_key).unwrap(), None);
    assert_eq!(indexes(&db), vec![expiry::expiry_row(&extended).unwrap()]);
}

#[test]
fn must_pass_reopen_recovers_legacy_primary_and_orphan_index_57() {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let row = message(2, 1_000);
    let orphan = message(3, 1_000);
    {
        let db = database(dir.path());
        primary(&db, &row);
        db.put_batch_pressure_bypass(cf::CF_KV, [expiry::expiry_row(&orphan).unwrap()])
            .unwrap();
        assert_eq!(indexes(&db).len(), 1);
    }
    let db = database(dir.path());
    assert!(
        db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes())
            .unwrap()
            .is_some()
    );
    assert_eq!(indexes(&db).len(), 1);
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        1
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes()).unwrap(),
        None
    );
    assert_eq!(indexes(&db).len(), 0);
}

#[test]
fn must_fail_corrupt_primary_cannot_authorize_indexed_deletion_57() {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    let row = message(4, 1_000);
    let bytes = b"not-json".to_vec();
    db.put_batch_pressure_bypass(
        cf::CF_KV,
        [
            (row.row_key.as_bytes().to_vec(), bytes.clone()),
            expiry::expiry_row(&row).unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes()).unwrap(),
        Some(bytes.clone())
    );
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        0
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes()).unwrap(),
        Some(bytes)
    );
    assert_eq!(indexes(&db).len(), 0);
}

#[test]
fn must_fail_interrupted_expiry_batch_keeps_both_rows_for_reopen_retry_57() {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let row = message(5, 1_000);
    let index = expiry::expiry_row(&row).unwrap();
    {
        let db = database(dir.path());
        primary(&db, &row);
        db.put_batch_pressure_bypass(cf::CF_KV, [index.clone()])
            .unwrap();
        let before = db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes()).unwrap();
        assert_eq!(indexes(&db), vec![index.clone()]);
        // NEEDS-SEAM: per-Db one-shot fault immediately BEFORE matching delete batch commit.
        db.fail_next_delete_batch_before_commit_for_test(cf::CF_KV, row.row_key.as_bytes());
        let error = cleanup_expired_mailbox_rows(&db, NOW + 1_000).unwrap_err();
        assert_eq!(
            error.data.as_ref().unwrap()["code"],
            error_codes::STORAGE_WRITE_FAILED
        );
        assert_eq!(
            db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes()).unwrap(),
            before
        );
        assert_eq!(indexes(&db), vec![index.clone()]);
    }
    let db = database(dir.path());
    assert!(
        db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes())
            .unwrap()
            .is_some()
    );
    assert_eq!(indexes(&db), vec![index]);
    assert_eq!(
        cleanup_expired_mailbox_rows(&db, NOW + 1_000)
            .unwrap()
            .deleted,
        1
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, row.row_key.as_bytes()).unwrap(),
        None
    );
    assert_eq!(indexes(&db).len(), 0);
}
