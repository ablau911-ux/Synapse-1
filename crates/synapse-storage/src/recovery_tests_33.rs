//! Issue #33: after RocksDB latches a background error (a failed WAL switch),
//! the batcher must return to healthy once the fault clears, without a daemon
//! restart. RocksDB has no resume call in the C API, so recovery is a reopen.

use super::*;
use std::{error::Error, time::Duration, time::Instant};

const TEST_SCHEMA_VERSION: u32 = 7;
const RECOVERY_DEADLINE: Duration = Duration::from_secs(60);

fn kv(key: &str, value: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![(key.as_bytes().to_vec(), value.as_bytes().to_vec())]
}

#[test]
fn must_pass_batcher_recovers_after_latched_rocksdb_error_clears() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let dir = temp.path().join("db");
    let db = Db::open(&dir, TEST_SCHEMA_VERSION)?;
    db.put_batch(cf::CF_KV, kv("before", "1"))?;
    db.flush()?;

    // Fault: no new files may be created, so the WAL switch of a flush fails
    // and RocksDB latches the error.
    let guard = crate::open_tests::deny_current_user_write(&dir)?;
    {
        let raw = db.raw()?;
        let handle = raw.cf_handle(cf::CF_KV).ok_or("CF_KV handle missing")?;
        assert!(
            raw.flush_cf(&handle).is_err(),
            "fault injection must make the memtable flush fail"
        );
    }
    let during = db.put_batch(cf::CF_KV, kv("during", "x"));
    println!(
        "regression_state=storage_recovery phase=fault write_err={:?} status={:?}",
        during.as_ref().err().map(ToString::to_string),
        db.batcher.status()
    );
    assert_eq!(
        during.as_ref().err().map(StorageError::code),
        Some(synapse_core::error_codes::STORAGE_WRITE_FAILED),
        "a write under a latched error must fail as a write failure"
    );
    assert!(db.batcher.status().failed, "batcher must report degraded");

    // The fault clears. No restart.
    drop(guard);
    let started = Instant::now();
    let mut recovered = false;
    while started.elapsed() < RECOVERY_DEADLINE {
        if db.put_batch(cf::CF_KV, kv("after", "2")).is_ok() {
            recovered = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let status = db.batcher.status();
    println!(
        "regression_state=storage_recovery phase=cleared recovered={recovered} after_ms={} status={status:?}",
        started.elapsed().as_millis()
    );
    assert!(
        recovered,
        "writes must succeed again after the fault clears"
    );
    assert!(!status.failed, "health must return to ok: {status:?}");
    assert!(status.recoveries >= 1);
    db.flush()?;

    let rows = db.scan_cf(cf::CF_KV)?;
    let has = |key: &str| rows.iter().any(|(k, _)| k == key.as_bytes());
    assert!(has("before"), "pre-fault row must survive the reopen");
    assert!(has("after"), "post-recovery row must be stored");
    assert!(!has("during"), "a refused write must not appear as a row");
    Ok(())
}

#[test]
fn must_fail_writes_stay_refused_and_degraded_while_the_fault_persists()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let dir = temp.path().join("db");
    let db = Db::open(&dir, TEST_SCHEMA_VERSION)?;
    db.put_batch(cf::CF_KV, kv("before", "1"))?;
    db.flush()?;

    let guard = crate::open_tests::deny_current_user_write(&dir)?;
    {
        let raw = db.raw()?;
        let handle = raw.cf_handle(cf::CF_KV).ok_or("CF_KV handle missing")?;
        assert!(
            raw.flush_cf(&handle).is_err(),
            "fault injection must make the memtable flush fail"
        );
    }
    // The fault never clears in this window: recovery must not fake health.
    let mut refused = 0_usize;
    for attempt in 0..8 {
        let key = format!("during-{attempt}");
        if let Err(error) = db.put_batch(cf::CF_KV, kv(&key, "x")) {
            assert_held_fault_refusal(attempt, &error);
            refused += 1;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let status = db.batcher.status();
    println!(
        "regression_state=storage_recovery phase=fault_held refused={refused} status={status:?}"
    );
    assert_eq!(refused, 8, "every write under a held fault must be refused");
    assert!(
        status.failed,
        "batcher must stay degraded while the fault holds"
    );

    drop(guard);
    let started = Instant::now();
    while started.elapsed() < RECOVERY_DEADLINE {
        if db.put_batch(cf::CF_KV, kv("after", "2")).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    db.flush()?;
    let rows = db.scan_cf(cf::CF_KV)?;
    assert!(
        !rows.iter().any(|(key, _)| key.starts_with(b"during-")),
        "refused writes must never surface as rows after recovery"
    );
    assert!(rows.iter().any(|(key, _)| key == b"before"));
    Ok(())
}

/// Refusal contract while the fault holds: the first write fails as a write
/// failure (latched RocksDB error); once a recovery reopen has failed, writes
/// are refused as an open failure naming the in-progress reopen. Nothing else
/// is acceptable.
fn assert_held_fault_refusal(attempt: usize, error: &StorageError) {
    let code = error.code();
    if attempt == 0 {
        assert_eq!(
            code,
            synapse_core::error_codes::STORAGE_WRITE_FAILED,
            "first write under the fault: {error}"
        );
        return;
    }
    if code == synapse_core::error_codes::STORAGE_OPEN_FAILED {
        assert!(
            error
                .to_string()
                .contains("storage is reopening after a failed recovery attempt"),
            "attempt {attempt}: OPEN_FAILED must name the failed recovery reopen: {error}"
        );
    } else {
        assert_eq!(
            code,
            synapse_core::error_codes::STORAGE_WRITE_FAILED,
            "attempt {attempt}: {error}"
        );
    }
}
