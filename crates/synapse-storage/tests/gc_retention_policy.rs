use std::{error::Error, sync::Arc};

use rocksdb::{DB, Options};
use synapse_core::error_codes;
use synapse_storage::{CrossCfBatchMutation, Db, cf};
use tempfile::TempDir;

mod support;
use support::synthetic_bytes;

#[test]
fn must_pass_default_gc_preserves_no_ttl_kv_rows_above_soft_cap() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    let rows = (0u64..16)
        .map(|index| {
            (
                format!("issue34/{index:02}").into_bytes(),
                synthetic_bytes(index + 1, 1024 * 1024),
            )
        })
        .collect::<Vec<_>>();
    db.put_batch(cf::CF_KV, rows)?;
    db.flush()?;
    db.compact_cf(cf::CF_KV)?;
    let before = db.scan_cf(cf::CF_KV)?;
    assert_eq!(before.len(), 16);

    let report = db.run_gc_once()?;
    let kv = report.cf(cf::CF_KV).ok_or("CF_KV report missing")?;
    assert!(kv.before_value > 10 * 1024 * 1024);
    let after = db.scan_cf(cf::CF_KV)?;
    assert_eq!(kv.evicted_rows, 0);
    assert_eq!(after, before);
    Ok(())
}

/// A hard-cap refusal must carry the hard-cap code, so an unrelated failure
/// (write failed, shed, quota) cannot satisfy the case.
fn assert_hard_cap<T: std::fmt::Debug>(result: synapse_storage::StorageResult<T>, context: &str) {
    match result {
        Ok(value) => panic!("{context}: expected STORAGE_CF_HARD_CAP_REACHED, got Ok({value:?})"),
        Err(error) => assert_eq!(
            error.code(),
            error_codes::STORAGE_CF_HARD_CAP_REACHED,
            "{context}: {error}"
        ),
    }
}

#[test]
fn must_fail_kv_hard_cap_refuses_normal_and_bypass_writes_across_boundary()
-> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    let rows = (0u64..49)
        .map(|index| {
            (
                format!("issue34/cap/{index:02}").into_bytes(),
                synthetic_bytes(index + 100, 1024 * 1024),
            )
        })
        .collect::<Vec<_>>();
    db.put_batch(cf::CF_KV, rows)?;
    db.flush()?;
    db.compact_cf(cf::CF_KV)?;
    assert!(db.cf_live_data_size_estimates()?.0[cf::CF_KV] < 50 * 1024 * 1024);
    let before = db.scan_cf(cf::CF_KV)?;
    assert_eq!(before.len(), 49);

    let normal_key = b"issue34/cap/normal";
    let bypass_key = b"issue34/cap/bypass";
    let normal_value = synthetic_bytes(600, 2 * 1024 * 1024);
    let bypass_value = synthetic_bytes(601, 2 * 1024 * 1024);
    assert_hard_cap(
        db.put_batch(cf::CF_KV, [(normal_key, normal_value)]),
        "normal write",
    );
    assert_hard_cap(
        db.put_batch_pressure_bypass(cf::CF_KV, [(bypass_key, bypass_value)]),
        "pressure-bypass write",
    );
    db.flush()?;
    assert_eq!(
        db.scan_cf(cf::CF_KV)?,
        before,
        "refused writes must leave CF_KV byte-for-byte unchanged"
    );
    Ok(())
}

#[test]
fn must_fail_kv_hard_cap_counts_uncompacted_writes() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    let rows = (0u64..49)
        .map(|index| {
            (
                format!("issue34/uncompacted/{index:02}").into_bytes(),
                synthetic_bytes(index + 700, 1024 * 1024),
            )
        })
        .collect::<Vec<_>>();
    db.put_batch(cf::CF_KV, rows)?;
    db.flush()?;
    let before = db.scan_cf(cf::CF_KV)?;
    assert_eq!(before.len(), 49);

    let key = b"issue34/uncompacted/cross";
    assert_hard_cap(
        db.put_batch(cf::CF_KV, [(key, synthetic_bytes(800, 2 * 1024 * 1024))]),
        "uncompacted crossing write",
    );
    db.flush()?;
    assert_eq!(db.scan_cf(cf::CF_KV)?, before);
    Ok(())
}

#[test]
fn must_pass_kv_hard_cap_allows_same_size_replacement_near_boundary() -> Result<(), Box<dyn Error>>
{
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    let key = b"issue34/replacement";
    let original = synthetic_bytes(900, 49 * 1024 * 1024);
    db.put_batch(cf::CF_KV, [(key, original)])?;
    db.flush()?;
    let replacement = synthetic_bytes(901, 49 * 1024 * 1024);
    db.put_batch(cf::CF_KV, [(key, replacement.clone())])?;
    db.flush()?;
    let rows = db.scan_cf(cf::CF_KV)?;
    assert_eq!(rows, vec![(key.to_vec(), replacement)]);
    Ok(())
}

#[test]
fn must_fail_kv_hard_cap_preserves_atomic_mutation_neighbors() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    let old_key = b"issue34/atomic/old";
    let new_key = b"issue34/atomic/new";
    db.put_batch(
        cf::CF_KV,
        [(old_key, synthetic_bytes(1000, 49 * 1024 * 1024))],
    )?;
    db.flush()?;

    let replacement = synthetic_bytes(1001, 49 * 1024 * 1024);
    db.mutate_batch_pressure_bypass(cf::CF_KV, [old_key], [(new_key, replacement.clone())])?;
    let rows = db.scan_cf(cf::CF_KV)?;
    assert_eq!(rows, vec![(new_key.to_vec(), replacement)]);

    let crossing_key = b"issue34/atomic/cross";
    let peer_key = b"issue34/atomic/peer";
    let result = db.mutate_cross_cf_batch_pressure_bypass([
        CrossCfBatchMutation::Put {
            cf_name: cf::CF_KV,
            key: crossing_key.to_vec(),
            value: synthetic_bytes(1002, 2 * 1024 * 1024),
        },
        CrossCfBatchMutation::Put {
            cf_name: cf::CF_SESSIONS,
            key: peer_key.to_vec(),
            value: b"must not commit".to_vec(),
        },
    ]);
    assert_hard_cap(result, "cross-CF mutation crossing the KV cap");
    assert_eq!(db.scan_cf(cf::CF_KV)?, rows);
    assert!(db.scan_cf(cf::CF_SESSIONS)?.is_empty());
    Ok(())
}

#[test]
fn must_pass_kv_hard_cap_uses_live_size_for_existing_compressed_rows() -> Result<(), Box<dyn Error>>
{
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    let compressible = vec![b'A'; 49 * 1024 * 1024];
    db.put_batch(
        cf::CF_KV,
        [(b"issue34/compressed".as_slice(), compressible)],
    )?;
    db.flush()?;
    db.compact_cf(cf::CF_KV)?;
    let estimate = db.cf_live_data_size_estimates()?.0[cf::CF_KV];
    assert!(estimate < 1024 * 1024, "estimate was {estimate}");

    let key = b"issue34/compressed/fresh";
    let fresh = synthetic_bytes(1100, 2 * 1024 * 1024);
    db.put_batch(cf::CF_KV, [(key, fresh.clone())])?;
    db.flush()?;
    assert!(
        db.scan_cf(cf::CF_KV)?
            .iter()
            .any(|(row_key, value)| row_key == key && value == &fresh)
    );
    Ok(())
}

#[test]
fn must_fail_kv_hard_cap_serializes_concurrent_admission() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = Arc::new(Db::open(&temp.path().join("db"), 1)?);
    let rows = (0u64..47)
        .map(|index| {
            (
                format!("issue34/concurrent/{index:02}").into_bytes(),
                synthetic_bytes(index + 1200, 1024 * 1024),
            )
        })
        .collect::<Vec<_>>();
    db.put_batch(cf::CF_KV, rows)?;
    db.flush()?;
    let outcomes = (0u64..2)
        .map(|index| {
            let db = Arc::clone(&db);
            std::thread::spawn(move || {
                db.put_batch(
                    cf::CF_KV,
                    [(
                        format!("issue34/concurrent/new/{index}").into_bytes(),
                        synthetic_bytes(index + 1300, 2 * 1024 * 1024),
                    )],
                )
            })
        })
        .map(|thread| thread.join().map_err(|_| "writer panicked"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    let refused = outcomes
        .into_iter()
        .find_map(Result::err)
        .ok_or("no refusal")?;
    assert_eq!(refused.code(), error_codes::STORAGE_CF_HARD_CAP_REACHED);
    db.flush()?;
    assert_eq!(db.scan_cf(cf::CF_KV)?.len(), 48);
    Ok(())
}

#[test]
fn must_pass_kv_hard_cap_allows_shrinking_an_existing_over_cap_row() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let path = temp.path().join("db");
    let key = b"issue34/shrink";
    let db = Db::open(&path, 1)?;
    db.put_batch(cf::CF_KV, [(key, synthetic_bytes(1600, 49 * 1024 * 1024))])?;
    db.flush()?;
    drop(db);

    let raw = DB::open_cf(&Options::default(), &path, cf::ALL_COLUMN_FAMILIES)?;
    let kv = raw.cf_handle(cf::CF_KV).ok_or("KV handle missing")?;
    raw.put_cf(&kv, key, synthetic_bytes(1601, 51 * 1024 * 1024))?;
    raw.flush_cf(&kv)?;
    raw.compact_range_cf(&kv, None::<&[u8]>, None::<&[u8]>);
    drop(kv);
    drop(raw);

    let db = Db::open(&path, 1)?;
    let smaller = synthetic_bytes(1602, 49 * 1024 * 1024);
    db.put_batch_pressure_bypass(cf::CF_KV, [(key, smaller.clone())])?;
    assert_eq!(db.scan_cf(cf::CF_KV)?, vec![(key.to_vec(), smaller)]);
    Ok(())
}
