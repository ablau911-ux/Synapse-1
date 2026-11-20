//! #34 regression: a quota reservation must survive a concurrent slow-path
//! reconcile. Writer A reserves hard-cap room, then is held before its batch
//! reaches the batcher; writer B takes the slow path (flush + exact recount).
//! Both near-cap admissions must never succeed together.

use std::{
    error::Error,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use tempfile::TempDir;

use crate::{Db, StorageError, StorageResult, cf};

const MIB: usize = 1024 * 1024;
const KV_HARD_CAP_BYTES: u64 = 50 * 1024 * 1024;

fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

/// CF_KV seeded to 47 MiB of the 50 MiB hard cap: one 2 MiB batch fits, two do not.
fn seeded_db(temp: &TempDir) -> Result<Arc<Db>, Box<dyn Error>> {
    let db = Arc::new(Db::open(&temp.path().join("db"), 1)?);
    let rows = (0u64..47)
        .map(|index| {
            (
                format!("issue34/seed/{index:02}").into_bytes(),
                bytes(index + 1, MIB),
            )
        })
        .collect::<Vec<_>>();
    db.put_batch(cf::CF_KV, rows)?;
    db.flush()?;
    Ok(db)
}

fn write_two_mib(db: &Db, index: u64) -> StorageResult<()> {
    db.put_batch(
        cf::CF_KV,
        [(
            format!("issue34/race/{index}").into_bytes(),
            bytes(index + 100, 2 * MIB),
        )],
    )
}

fn persisted_kv_bytes(db: &Db) -> Result<u64, Box<dyn Error>> {
    db.flush()?;
    Ok(db
        .scan_cf(cf::CF_KV)?
        .iter()
        .map(|(key, value)| (key.len() + value.len()) as u64)
        .sum())
}

#[test]
fn must_fail_paused_reservation_is_not_forgotten_by_a_concurrent_slow_path()
-> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = seeded_db(&temp)?;
    let (reserved_tx, reserved_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let calls = AtomicUsize::new(0);
    db.set_reservation_hook(Arc::new(move || {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = reserved_tx.send(());
            if let Ok(release) = release_rx.lock() {
                let _ = release.recv();
            }
        }
    }));

    let writer_a = {
        let db = Arc::clone(&db);
        thread::spawn(move || write_two_mib(&db, 0))
    };
    reserved_rx.recv_timeout(Duration::from_secs(60))?;

    let (b_tx, b_rx) = mpsc::channel();
    let writer_b = {
        let db = Arc::clone(&db);
        thread::spawn(move || {
            let _ = b_tx.send(write_two_mib(&db, 1));
        })
    };
    // Unfixed code lets B finish while A is paused; fixed code makes B wait
    // for A's reservation, so B only completes after A is released.
    let early_b = b_rx.recv_timeout(Duration::from_secs(3)).ok();
    release_tx.send(())?;
    let result_a = writer_a.join().map_err(|_| "writer A panicked")?;
    let result_b = match early_b {
        Some(result) => result,
        None => b_rx.recv_timeout(Duration::from_secs(60))?,
    };
    writer_b.join().map_err(|_| "writer B panicked")?;

    let accepted = [&result_a, &result_b]
        .iter()
        .filter(|result| result.is_ok())
        .count();
    assert_eq!(
        accepted, 1,
        "A={result_a:?} B={result_b:?}: both near-cap admissions succeeded"
    );
    assert!(
        [&result_a, &result_b]
            .iter()
            .any(|result| matches!(result, Err(StorageError::HardCapReached { .. })))
    );
    assert!(persisted_kv_bytes(&db)? <= KV_HARD_CAP_BYTES);
    Ok(())
}

#[test]
fn must_fail_simultaneous_writers_cannot_jointly_exceed_the_hard_cap() -> Result<(), Box<dyn Error>>
{
    let temp = TempDir::new()?;
    let db = seeded_db(&temp)?;
    let writers = 8_u64;
    let barrier = Arc::new(Barrier::new(writers as usize));
    let handles = (0..writers)
        .map(|index| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                write_two_mib(&db, index)
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().map_err(|_| "writer panicked"))
        .collect::<Result<Vec<_>, _>>()?;
    let accepted = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(accepted, 1, "{results:?}");
    for rejected in results.iter().filter_map(|result| result.as_ref().err()) {
        assert!(
            matches!(rejected, StorageError::HardCapReached { .. }),
            "every losing writer must be refused by the hard cap, got {rejected:?}"
        );
    }
    assert!(persisted_kv_bytes(&db)? <= KV_HARD_CAP_BYTES);
    Ok(())
}

#[test]
fn must_pass_single_near_cap_writer_is_admitted_and_persisted() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = seeded_db(&temp)?;
    let before = persisted_kv_bytes(&db)?;
    write_two_mib(&db, 0)?;
    let key = b"issue34/race/0";
    let after = persisted_kv_bytes(&db)?;
    assert_eq!(after, before + (key.len() + 2 * MIB) as u64);
    assert!(after <= KV_HARD_CAP_BYTES);
    assert_eq!(db.get_cf_exact(cf::CF_KV, key)?, Some(bytes(100, 2 * MIB)));
    Ok(())
}

#[test]
fn must_pass_admitted_reservation_is_released_so_later_small_writes_fit()
-> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = seeded_db(&temp)?;
    write_two_mib(&db, 0)?;
    // 47 MiB + 2 MiB persisted leaves room for 1 KiB: the admitted batch's
    // reservation must not keep counting against the cap after it lands.
    db.put_batch(
        cf::CF_KV,
        [(b"issue34/after/small".to_vec(), bytes(7, 1024))],
    )?;
    assert!(persisted_kv_bytes(&db)? <= KV_HARD_CAP_BYTES);
    Ok(())
}

#[test]
fn must_pass_rewriting_the_same_near_cap_key_is_not_double_counted() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let db = seeded_db(&temp)?;
    write_two_mib(&db, 0)?;
    let before = persisted_kv_bytes(&db)?;
    // Same key and size: the projected delta is zero, so the cap must admit it.
    write_two_mib(&db, 0)?;
    assert_eq!(persisted_kv_bytes(&db)?, before);
    Ok(())
}
