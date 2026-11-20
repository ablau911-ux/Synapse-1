//! Small durable coordination writes must not rotate one WAL per call (#64).

use std::fs;

use synapse_core::SCHEMA_VERSION;
use synapse_storage::{Db, cf};

#[test]
fn must_pass_small_bypass_writes_reuse_wal_and_survive_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let db = Db::open(&path, SCHEMA_VERSION)?;
    let before = wal_count(&path)?;

    for index in 0..12 {
        let key = format!("syn64-wal-{index:02}");
        db.put_batch_pressure_bypass(
            cf::CF_KV,
            [(key.as_bytes().to_vec(), b"synthetic-value".to_vec())],
        )?;
    }
    let after = wal_count(&path)?;
    assert!(
        after <= before + 1,
        "12 small writes grew WAL files from {before} to {after}"
    );
    drop(db);

    let reopened = Db::open(&path, SCHEMA_VERSION)?;
    for index in 0..12 {
        let key = format!("syn64-wal-{index:02}");
        let rows = reopened.scan_cf_prefix(cf::CF_KV, key.as_bytes())?;
        assert_eq!(rows, vec![(key.into_bytes(), b"synthetic-value".to_vec())]);
    }
    Ok(())
}

#[test]
fn must_fail_refused_or_empty_bypass_writes_rotate_no_wal_and_leave_no_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let db = Db::open(&path, SCHEMA_VERSION)?;
    let before = wal_count(&path)?;

    for name in ["", "CF_NOT_DECLARED", "cf_kv"] {
        let result = db.put_batch_pressure_bypass(
            name,
            [(b"syn64-refused".to_vec(), b"synthetic-value".to_vec())],
        );
        match result {
            Ok(()) => panic!("bypass write to undeclared CF {name:?} must be refused"),
            Err(error) => assert_eq!(
                error.code(),
                synapse_core::error_codes::STORAGE_WRITE_FAILED,
                "{name:?}: {error}"
            ),
        }
    }
    for _ in 0..12 {
        db.put_batch_pressure_bypass(cf::CF_KV, Vec::<(Vec<u8>, Vec<u8>)>::new())?;
    }
    let after = wal_count(&path)?;
    assert_eq!(
        after, before,
        "refused and empty bypass writes must not touch the WAL"
    );
    assert!(db.scan_cf(cf::CF_KV)?.is_empty());
    drop(db);

    let reopened = Db::open(&path, SCHEMA_VERSION)?;
    assert!(reopened.scan_cf(cf::CF_KV)?.is_empty());
    Ok(())
}

fn wal_count(path: &std::path::Path) -> std::io::Result<usize> {
    Ok(fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .count())
}
