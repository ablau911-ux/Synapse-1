//! Regression tests for #57: one corrupt mailbox row must not fail every send.
//! The global expiry sweep skips and counts undecodable / wrong-schema rows.

use tempfile::TempDir;

use super::mailbox_sweep_throttle_tests::{message_rows, register, send, service};
use super::*;

const NOW: u64 = 10_000;

fn corrupt_key(label: &str) -> Vec<u8> {
    format!("{}corrupt-{label}", mailbox_recipient_prefix("victim")).into_bytes()
}

fn put_garbage_row(db: &Db, label: &str) -> anyhow::Result<Vec<u8>> {
    let key = corrupt_key(label);
    db.put_batch(cf::CF_KV, [(key.clone(), b"{not json".to_vec())])?;
    Ok(key)
}

fn put_wrong_schema_row(db: &Db, source_key: &[u8], label: &str) -> anyhow::Result<Vec<u8>> {
    let encoded = db
        .get_cf_exact(cf::CF_KV, source_key)?
        .ok_or_else(|| anyhow::anyhow!("source row missing"))?;
    let mut value: Value = synapse_storage::decode_json(&encoded)?;
    value["schema_version"] = json!(SCHEMA_VERSION + 1000);
    let key = corrupt_key(label);
    db.put_batch(
        cf::CF_KV,
        [(key.clone(), synapse_storage::encode_json(&value)?)],
    )?;
    Ok(key)
}

fn seeded() -> anyhow::Result<(TempDir, SynapseService)> {
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for session in ["sender", "r1", "r2"] {
        register(&service, session, 1_000)?;
    }
    Ok((temp, service))
}

#[test]
fn must_pass_send_succeeds_when_a_mailbox_row_is_undecodable() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let db = service.mailbox_db()?;
    let key = put_garbage_row(&db, "garbage-send")?;

    let response = send(&service, "r1", 600_000, NOW)?;
    assert_eq!(response.to_session, "r1");
    assert!(
        db.get_cf_exact(cf::CF_KV, &key)?.is_some(),
        "the corrupt row is left in place for inspection, not deleted"
    );
    assert_eq!(message_rows(&db)?, 2, "corrupt row plus the new message");
    Ok(())
}

#[test]
fn must_fail_sweep_counts_corrupt_rows_and_still_reclaims_expired_ones() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let db = service.mailbox_db()?;
    send(&service, "r2", 1_000, NOW)?;
    let source = db
        .scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("seed message missing"))?
        .0;
    put_garbage_row(&db, "garbage-count")?;
    put_wrong_schema_row(&db, &source, "schema-count")?;

    let sweep = cleanup_expired_mailbox_rows(&db, NOW + 5_000)?;
    assert_eq!(
        sweep,
        MailboxSweep {
            deleted: 1,
            corrupt_skipped: 2
        }
    );
    assert_eq!(message_rows(&db)?, 2, "only the corrupt rows remain");
    Ok(())
}

#[test]
fn must_fail_throttled_sweep_reports_the_cached_corrupt_count_inside_the_interval()
-> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let db = service.mailbox_db()?;
    assert!(mailbox_sweep_interval_ms() > 0);
    put_garbage_row(&db, "garbage-throttle")?;

    let first = throttled_cleanup_expired_mailbox_rows(&db, NOW);
    assert_eq!(first.corrupt_skipped, 1);
    // A second corrupt row planted inside the interval must NOT be counted:
    // the throttled call returns the cached sweep, it does not rescan.
    put_garbage_row(&db, "garbage-throttle-late")?;
    let second = throttled_cleanup_expired_mailbox_rows(&db, NOW + 1);
    assert_eq!(
        second,
        MailboxSweep {
            deleted: 0,
            corrupt_skipped: 1
        }
    );
    Ok(())
}

#[test]
fn must_fail_corrupt_row_is_logged_once_per_key() {
    let key = corrupt_key("log-once-unique");
    assert!(note_corrupt_mailbox_row(&key, "first"));
    assert!(!note_corrupt_mailbox_row(&key, "again"));
}
