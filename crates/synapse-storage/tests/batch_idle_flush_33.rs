//! A lone writer must not wait for a coalescing timer, and concurrent writers
//! must not be serialized behind one another or spuriously refused (#33).

use std::{error::Error, sync::Arc, time::Instant};

use synapse_storage::{Db, cf};

const TEST_SCHEMA_VERSION: u32 = 7;

#[test]
fn must_pass_sequential_single_row_writes_do_not_wait_for_the_flush_timer()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let started = Instant::now();
    for index in 0..20_u32 {
        db.put_batch(
            cf::CF_ACTION_LOG,
            [(format!("seq-{index:04}").into_bytes(), b"x".to_vec())],
        )?;
    }
    let elapsed = started.elapsed();
    // Each write used to block ~100 ms on the timer (2 s for 20 writes).
    assert!(
        elapsed.as_millis() < 1_000,
        "20 sequential writes took {elapsed:?}"
    );
    assert_eq!(db.scan_cf(cf::CF_ACTION_LOG)?.len(), 20);
    assert!(!db.batcher_status().failed);
    Ok(())
}

#[test]
fn must_pass_concurrent_writers_all_succeed_and_persist() -> Result<(), Box<dyn Error>> {
    const WRITERS: usize = 16;
    const WRITES_EACH: usize = 10;
    let temp = tempfile::tempdir()?;
    let db = Arc::new(Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?);
    let started = Instant::now();
    let results = std::thread::scope(|scope| {
        let handles = (0..WRITERS)
            .map(|writer| {
                let db = Arc::clone(&db);
                scope.spawn(move || -> Result<(), String> {
                    for index in 0..WRITES_EACH {
                        db.put_batch(
                            cf::CF_ACTION_LOG,
                            [(
                                format!("w{writer:02}-{index:03}").into_bytes(),
                                b"v".to_vec(),
                            )],
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(std::thread::ScopedJoinHandle::join)
            .collect::<Vec<_>>()
    });
    for result in results {
        result
            .map_err(|_| std::io::Error::other("writer panicked"))?
            .map_err(std::io::Error::other)?;
    }
    // 160 writes at ~100 ms each would take 16 s if serialized on the timer.
    assert!(
        started.elapsed().as_secs() < 8,
        "concurrent writes took {:?}",
        started.elapsed()
    );
    assert_eq!(db.scan_cf(cf::CF_ACTION_LOG)?.len(), WRITERS * WRITES_EACH);
    Ok(())
}

#[test]
fn must_fail_refused_writes_do_not_persist_degrade_or_stall_the_batcher()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;

    let undeclared = db.put_batch("CF_NOT_DECLARED", [(b"probe".to_vec(), b"x".to_vec())]);
    assert_eq!(
        undeclared.as_ref().err().map(|error| error.code()),
        Some(synapse_core::error_codes::STORAGE_WRITE_FAILED),
        "a write to an undeclared CF must be refused as a write failure"
    );
    // One request is bounded to 64 MiB including allocation overhead.
    let oversized = db.put_batch(
        cf::CF_ACTION_LOG,
        [(b"oversized".to_vec(), vec![0_u8; 65 * 1024 * 1024])],
    );
    assert_eq!(
        oversized.as_ref().err().map(|error| error.code()),
        Some(synapse_core::error_codes::STORAGE_WRITE_FAILED),
        "a batch over the 64 MiB bound must be refused as a write failure"
    );

    assert!(
        db.scan_cf(cf::CF_ACTION_LOG)?.is_empty(),
        "a refused write must not leave a row behind"
    );
    let status = db.batcher_status();
    assert!(
        !status.failed,
        "caller-side refusals must not latch the batcher as degraded: {status:?}"
    );

    let started = Instant::now();
    db.put_batch(cf::CF_ACTION_LOG, [(b"after".to_vec(), b"v".to_vec())])?;
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 1_000,
        "the next lone write must not be stalled by earlier refusals: {elapsed:?}"
    );
    let rows = db.scan_cf(cf::CF_ACTION_LOG)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, b"after".to_vec());
    Ok(())
}
