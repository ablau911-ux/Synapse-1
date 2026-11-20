//! Regression for #164: CF_KV used to have one shared 50 MiB hard cap, so
//! mailbox or workspace data could fill it and starve approvals, sessions and
//! other critical rows. Namespaces are now bounded separately and critical
//! namespaces can always write.

use std::error::Error;

use synapse_storage::{Db, StorageError, cf};

mod support;
use support::synthetic_bytes;

const TEST_SCHEMA_VERSION: u32 = 7;
const QUOTA_CODE: &str = "STORAGE_NAMESPACE_QUOTA_REACHED";
const HARD_CAP_CODE: &str = "STORAGE_CF_HARD_CAP_REACHED";
const MIB: usize = 1024 * 1024;
const MAILBOX_PREFIX: &str = "agent-mailbox/v1/recipient_hex/aa/";
const WORKSPACE_PREFIX: &str = "workspace-blackboard/v1/run/";
const CRITICAL_KEYS: [&str; 5] = [
    "mcp/session/v1/session-164",
    "approval/v1/item/approval-164",
    "escalation/v1/item/escalation-164",
    "m4/act_run_shell/idempotency/v1/global/key-164",
    "agent-task/v1/task/task-164",
];

fn put(db: &Db, key: &str, value_len: usize) -> Result<(), StorageError> {
    db.put_batch(
        cf::CF_KV,
        [(key.as_bytes().to_vec(), vec![7_u8; value_len])],
    )
}

fn assert_critical_writes_succeed(db: &Db) -> Result<(), Box<dyn Error>> {
    for key in CRITICAL_KEYS {
        put(db, key, 512).map_err(|error| format!("critical write {key} refused: {error}"))?;
    }
    db.flush()?;
    for key in CRITICAL_KEYS {
        let rows = db.scan_cf_prefix(cf::CF_KV, key.as_bytes())?;
        assert_eq!(rows.len(), 1, "critical row {key} must be readable");
    }
    Ok(())
}

/// A quota-refused key must leave no row behind.
fn assert_absent(db: &Db, key: &str) -> Result<(), Box<dyn Error>> {
    let rows = db.scan_cf_prefix(cf::CF_KV, key.as_bytes())?;
    assert!(
        rows.iter()
            .all(|(row_key, _value)| row_key != key.as_bytes()),
        "refused key {key} must not be persisted"
    );
    Ok(())
}

#[test]
fn must_fail_full_mailbox_namespace_is_refused_while_critical_and_workspace_writes_succeed()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;

    let mut accepted_bytes = 0_usize;
    let mut refusal = None;
    for index in 0..160_usize {
        let key = format!("{MAILBOX_PREFIX}{index:06}");
        match put(&db, &key, 256 * 1024) {
            Ok(()) => accepted_bytes += key.len() + 256 * 1024,
            Err(error) => {
                refusal = Some((key, error));
                break;
            }
        }
    }
    let (refused_key, refusal) =
        refusal.ok_or("mailbox namespace accepted 40 MiB without a quota refusal")?;
    assert_eq!(refusal.code(), QUOTA_CODE, "refusal: {refusal}");
    assert!(
        accepted_bytes <= 20 * MIB,
        "mailbox namespace kept {accepted_bytes} bytes, expected a bounded budget"
    );

    // The mailbox stays refused, but nothing else is starved.
    let again = put(&db, &format!("{MAILBOX_PREFIX}after-full"), 256 * 1024)
        .err()
        .ok_or("full mailbox accepted another large message")?;
    assert_eq!(again.code(), QUOTA_CODE);
    assert_critical_writes_succeed(&db)?;
    put(&db, &format!("{WORKSPACE_PREFIX}key-1"), 128 * 1024)
        .map_err(|error| format!("workspace write refused by mailbox quota: {error}"))?;
    db.flush()?;
    assert_absent(&db, &refused_key)?;
    assert_absent(&db, &format!("{MAILBOX_PREFIX}after-full"))?;
    Ok(())
}

#[test]
fn must_fail_full_workspace_namespace_is_refused_while_mailbox_and_critical_writes_succeed()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;

    let mut refusal = None;
    for index in 0..160_usize {
        let key = format!("{WORKSPACE_PREFIX}{index:06}");
        if let Err(error) = put(&db, &key, 256 * 1024) {
            refusal = Some((key, error));
            break;
        }
    }
    let (refused_key, refusal) =
        refusal.ok_or("workspace namespace accepted 40 MiB without a quota refusal")?;
    assert_eq!(refusal.code(), QUOTA_CODE, "refusal: {refusal}");

    assert_critical_writes_succeed(&db)?;
    put(&db, &format!("{MAILBOX_PREFIX}000001"), 64 * 1024)
        .map_err(|error| format!("mailbox write refused by workspace quota: {error}"))?;
    db.flush()?;
    assert_absent(&db, &refused_key)?;
    Ok(())
}

#[test]
fn must_pass_critical_namespaces_write_after_the_shared_cf_cap_is_exhausted()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;

    let mut refusal = None;
    // Incompressible fill: the shared cap counts on-disk (LZ4-compressed) live data.
    for index in 0..120_u64 {
        let key = format!("misc/v1/fill/{index:06}");
        if let Err(error) = db.put_batch(
            cf::CF_KV,
            [(key.into_bytes(), synthetic_bytes(index + 1, MIB))],
        ) {
            refusal = Some(error);
            break;
        }
    }
    let refusal = refusal.ok_or("uncategorized rows never hit the shared CF_KV cap")?;
    assert_eq!(refusal.code(), HARD_CAP_CODE, "refusal: {refusal}");

    // Less than one fill row of headroom remains, so a 2 MiB write must cross the cap.
    let uncategorized = db
        .put_batch(
            cf::CF_KV,
            [(
                b"misc/v1/fill/after-cap".to_vec(),
                synthetic_bytes(999, 2 * MIB),
            )],
        )
        .err()
        .ok_or("uncategorized write accepted past the shared cap")?;
    assert_eq!(uncategorized.code(), HARD_CAP_CODE);
    assert_critical_writes_succeed(&db)?;
    Ok(())
}
