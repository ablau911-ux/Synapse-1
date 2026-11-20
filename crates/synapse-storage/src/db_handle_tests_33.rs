//! Issue #33: `DbHandle::reopen` is the recovery primitive. It must keep
//! persisted rows, hand out a fresh database, and refuse (changing nothing)
//! while an in-flight user still holds the old one.

use super::*;
use std::error::Error;

const TEST_SCHEMA_VERSION: u32 = 7;

#[test]
fn must_pass_reopen_replaces_the_database_and_keeps_persisted_rows() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    db.put_batch(cf::CF_KV, vec![(b"k1".to_vec(), b"v1".to_vec())])?;
    db.flush()?;
    let old = db.raw()?;
    let old_addr = Arc::as_ptr(&old);
    drop(old);

    db.inner.reopen()?;

    let fresh = db.raw()?;
    assert_ne!(
        Arc::as_ptr(&fresh),
        old_addr,
        "reopen must create a new database"
    );
    drop(fresh);
    db.put_batch(cf::CF_KV, vec![(b"k2".to_vec(), b"v2".to_vec())])?;
    db.flush()?;
    let rows = db.scan_cf(cf::CF_KV)?;
    assert!(rows.iter().any(|(k, v)| k == b"k1" && v == b"v1"));
    assert!(rows.iter().any(|(k, v)| k == b"k2" && v == b"v2"));
    Ok(())
}

#[test]
fn must_fail_reopen_is_refused_while_the_database_is_in_use() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let held = db.raw()?;

    let result = db.inner.reopen();
    let Err(error) = result else {
        panic!("reopen must not close a database that is in use");
    };
    assert_eq!(
        error.code(),
        synapse_core::error_codes::STORAGE_OPEN_FAILED,
        "{error}"
    );
    assert!(
        error.to_string().contains("still in use"),
        "refusal must name the in-use reason: {error}"
    );
    let still = db.raw()?;
    assert!(
        Arc::ptr_eq(&held, &still),
        "a refused reopen must change nothing"
    );
    Ok(())
}
