//! A batch owner must receive the `RocksDB` write result, not an enqueue ack (#33).

use std::path::Path;
use std::sync::Arc;

use rocksdb::{DB, Options};
pub use synapse_storage::{StorageError, StorageResult};

#[path = "../src/batch.rs"]
mod batch;
#[path = "../src/db_handle.rs"]
mod db_handle;

const READ_ONLY_SCHEMA: u32 = 0;
const WRITABLE_SCHEMA: u32 = 1;

fn open_rocksdb(path: &Path, schema_version: u32) -> StorageResult<DB> {
    let mut options = Options::default();
    if schema_version == READ_ONLY_SCHEMA {
        DB::open_cf_for_read_only(&options, path, ["default"], false).map_err(|error| {
            StorageError::OpenFailed {
                path: path.to_path_buf(),
                detail: error.to_string(),
            }
        })
    } else {
        options.create_if_missing(true);
        DB::open_cf(&options, path, ["default"]).map_err(|error| StorageError::OpenFailed {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })
    }
}

/// A refusal must be the batcher's write failure, not an admission timeout
/// or any other error that would also satisfy a bare `is_err()`.
fn assert_write_failed<T: std::fmt::Debug>(result: StorageResult<T>, context: &str) {
    match result {
        Ok(value) => panic!("{context}: expected STORAGE_WRITE_FAILED, got Ok({value:?})"),
        Err(error) => assert_eq!(
            error.code(),
            synapse_core::error_codes::STORAGE_WRITE_FAILED,
            "{context}: {error}"
        ),
    }
}

#[test]
fn must_fail_read_only_failure_reaches_every_owner_and_later_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let mut options = Options::default();
    options.create_if_missing(true);
    let writable = DB::open_cf(&options, &path, ["default"])?;
    assert!(writable.cf_handle("default").is_some());
    writable.put(b"sentinel", b"unchanged")?;
    drop(writable);

    let read_only = open_rocksdb(&path, READ_ONLY_SCHEMA)?;
    assert!(read_only.cf_handle("default").is_some());
    let handle = Arc::new(db_handle::DbHandle::new(read_only, &path, READ_ONLY_SCHEMA));
    let batcher = batch::Batcher::spawn(Arc::clone(&handle));
    let results = std::thread::scope(|scope| {
        let mut owners = Vec::new();
        for index in 0..3 {
            let batcher = &batcher;
            owners.push(scope.spawn(move || {
                batcher.put_batch(
                    "default",
                    vec![(format!("probe-{index}").into_bytes(), b"value".to_vec())],
                )
            }));
        }
        owners
            .into_iter()
            .map(std::thread::ScopedJoinHandle::join)
            .collect::<Vec<_>>()
    });
    for (index, owner) in results.into_iter().enumerate() {
        let result = owner.map_err(|_| std::io::Error::other("write owner panicked"))?;
        assert_write_failed(result, &format!("write owner {index}"));
    }
    assert_write_failed(batcher.flush(), "flush after failed batch");
    let status = batcher.status();
    assert!(status.failed, "failed batcher reported healthy: {status:?}");
    assert!(status.detail.is_some());
    assert!(status.consecutive_failures >= 1);
    assert_eq!(status.recoveries, 0);
    assert!(status.retry_in_ms.is_some());
    assert_write_failed(
        batcher.put_batch("default", vec![(b"later".to_vec(), b"value".to_vec())]),
        "later write after persistent failure",
    );
    for index in 0..1000 {
        assert_write_failed(
            batcher.put_batch(
                "default",
                vec![(format!("later-{index}").into_bytes(), Vec::new())],
            ),
            &format!("later write {index}"),
        );
    }
    drop(batcher);
    drop(handle);

    let reopened = DB::open_cf(&options, &path, ["default"])?;
    assert_eq!(
        reopened.get(b"sentinel")?.as_deref(),
        Some(&b"unchanged"[..])
    );
    for key in [b"probe-0".as_slice(), b"probe-1", b"probe-2", b"later"] {
        assert!(reopened.get(key)?.is_none(), "failed write reached the DB");
    }
    Ok(())
}

#[test]
fn must_pass_writable_batch_accepts_small_and_boundary_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let db = open_rocksdb(&path, WRITABLE_SCHEMA)?;
    assert!(db.cf_handle("default").is_some());
    let handle = Arc::new(db_handle::DbHandle::new(db, &path, WRITABLE_SCHEMA));
    let batcher = batch::Batcher::spawn(Arc::clone(&handle));
    batcher.put_batch("default", Vec::new())?;
    batcher.put_batch("default", vec![(b"small".to_vec(), b"value".to_vec())])?;
    batcher.put_batch(
        "default",
        vec![(b"boundary".to_vec(), vec![b'x'; batch::FLUSH_BYTES + 1])],
    )?;
    batcher.flush()?;
    let db = handle.get()?;
    assert_eq!(db.get(b"small")?.as_deref(), Some(&b"value"[..]));
    assert_eq!(
        db.get(b"boundary")?.as_deref().map(<[u8]>::len),
        Some(batch::FLUSH_BYTES + 1)
    );
    Ok(())
}

#[test]
fn must_fail_oversized_batch_is_refused_without_rows_or_degrading()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let db = open_rocksdb(&path, WRITABLE_SCHEMA)?;
    let handle = Arc::new(db_handle::DbHandle::new(db, &path, WRITABLE_SCHEMA));
    let batcher = batch::Batcher::spawn(Arc::clone(&handle));
    let result = batcher.put_batch(
        "default",
        vec![(b"oversized".to_vec(), vec![b'x'; 65 * 1024 * 1024])],
    );
    let Err(error) = result else {
        panic!("oversized batch request was accepted");
    };
    assert_eq!(
        error.code(),
        synapse_core::error_codes::STORAGE_WRITE_FAILED,
        "{error}"
    );
    assert!(
        error.to_string().contains("byte limit"),
        "refusal must name the size limit: {error}"
    );
    batcher.flush()?;
    assert!(handle.get()?.get(b"oversized")?.is_none());
    let status = batcher.status();
    assert!(
        !status.failed,
        "a request-level size refusal must not degrade the batcher: {status:?}"
    );
    Ok(())
}

#[test]
fn must_pass_failed_batcher_recovers_after_backoff_when_a_real_write_succeeds()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let db = open_rocksdb(&path, WRITABLE_SCHEMA)?;
    assert!(db.cf_handle("default").is_some());
    let handle = Arc::new(db_handle::DbHandle::new(db, &path, WRITABLE_SCHEMA));
    let batcher = batch::Batcher::spawn(Arc::clone(&handle));
    assert!(!batcher.status().failed);

    // A write naming a missing column family fails the commit and latches.
    assert_write_failed(
        batcher.put_batch("missing-cf", vec![(b"k".to_vec(), b"v".to_vec())]),
        "write to a missing CF",
    );
    let failed = batcher.status();
    assert!(failed.failed, "{failed:?}");
    assert_eq!(failed.consecutive_failures, 1);

    // Inside the backoff window valid writes are still refused.
    assert_write_failed(
        batcher.put_batch("default", vec![(b"early".to_vec(), b"v".to_vec())]),
        "write inside the backoff window",
    );
    assert!(handle.get()?.get(b"early")?.is_none());

    // After the backoff the next real write is the recovery probe.
    std::thread::sleep(batch::RETRY_BACKOFF_INITIAL + std::time::Duration::from_millis(200));
    batcher.put_batch("default", vec![(b"probe".to_vec(), b"v".to_vec())])?;
    let recovered = batcher.status();
    assert!(!recovered.failed, "{recovered:?}");
    assert_eq!(recovered.recoveries, 1);
    assert_eq!(recovered.consecutive_failures, 0);
    assert_eq!(handle.get()?.get(b"probe")?.as_deref(), Some(&b"v"[..]));
    Ok(())
}

#[test]
fn must_fail_failed_probe_keeps_the_batcher_failed_and_backs_off_longer()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let db = open_rocksdb(&path, WRITABLE_SCHEMA)?;
    assert!(db.cf_handle("default").is_some());
    let handle = Arc::new(db_handle::DbHandle::new(db, &path, WRITABLE_SCHEMA));
    let batcher = batch::Batcher::spawn(Arc::clone(&handle));
    assert_write_failed(
        batcher.put_batch("missing-cf", vec![(b"a".to_vec(), b"v".to_vec())]),
        "first failing write",
    );
    let first_retry_ms = batcher
        .status()
        .retry_in_ms
        .ok_or("first failure must schedule a retry")?;
    std::thread::sleep(batch::RETRY_BACKOFF_INITIAL + std::time::Duration::from_millis(200));
    // The probe is a real write; it fails again, so the failure stays latched.
    assert_write_failed(
        batcher.put_batch("missing-cf", vec![(b"b".to_vec(), b"v".to_vec())]),
        "failed probe",
    );
    let status = batcher.status();
    assert!(status.failed, "{status:?}");
    assert_eq!(status.consecutive_failures, 2);
    assert_eq!(status.recoveries, 0);
    let second_retry_ms = status
        .retry_in_ms
        .ok_or("failed probe must schedule a retry")?;
    let initial_ms = u64::try_from(batch::RETRY_BACKOFF_INITIAL.as_millis())?;
    assert!(
        first_retry_ms <= initial_ms && second_retry_ms > initial_ms,
        "failed probe must back off longer than the initial {initial_ms} ms: \
         first={first_retry_ms} second={second_retry_ms}"
    );
    Ok(())
}
