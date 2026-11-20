use super::*;
use std::error::Error;

const TEST_SCHEMA_VERSION: u32 = 7;
#[test]
fn must_pass_batch_explicit_flush_round_trips_bytes() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let before = db.scan_cf(cf::CF_EVENTS)?;
    println!(
        "regression_state=cf_scan case=explicit_flush before_count={} before_bytes={}",
        before.len(),
        total_value_bytes(&before)
    );

    let expected = vec![
        (
            b"explicit-01".to_vec(),
            br#"{"ts_ns":1,"kind":"a"}"#.to_vec(),
        ),
        (
            b"explicit-02".to_vec(),
            br#"{"ts_ns":2,"kind":"b"}"#.to_vec(),
        ),
    ];
    db.put_batch(cf::CF_EVENTS, expected.clone())?;
    db.flush()?;

    let after = db.scan_cf(cf::CF_EVENTS)?;
    println!(
        "regression_state=cf_scan case=explicit_flush after_count={} after_bytes={} observed={:?}",
        after.len(),
        total_value_bytes(&after),
        printable_rows(&after)
    );
    assert_eq!(sorted_rows(after), sorted_rows(expected));
    Ok(())
}

#[test]
fn must_pass_batch_timer_flushes_after_interval() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let expected = vec![(b"timer-01".to_vec(), b"x".to_vec())];
    let before = db.scan_cf(cf::CF_ACTION_LOG)?;
    assert!(before.is_empty(), "fresh CF must start empty: {before:?}");

    // A sub-FLUSH_BYTES write with no explicit flush: `put_batch` returns only
    // once the batcher committed it, so the row must be readable the moment
    // it returns, and the commit must not wait for anything beyond the
    // batcher's own interval (generous 10x bound for loaded hosts).
    let started = std::time::Instant::now();
    db.put_batch(cf::CF_ACTION_LOG, expected.clone())?;
    let elapsed = started.elapsed();
    let after = db.scan_cf(cf::CF_ACTION_LOG)?;
    println!(
        "regression_state=cf_scan case=timer elapsed_ms={} after_count={} observed={:?}",
        elapsed.as_millis(),
        after.len(),
        printable_rows(&after)
    );
    assert_eq!(sorted_rows(after), sorted_rows(expected));
    assert!(
        elapsed < batch::FLUSH_INTERVAL.saturating_mul(10),
        "small write took {elapsed:?} to commit"
    );
    Ok(())
}

#[test]
fn must_pass_batch_edges_empty_single_byte_and_size_boundary() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;

    let before_empty = db.scan_cf(cf::CF_KV)?;
    let empty: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    db.put_batch(cf::CF_KV, empty)?;
    db.flush()?;
    let after_empty = db.scan_cf(cf::CF_KV)?;
    println!(
        "regression_state=cf_scan edge=empty before_count={} after_count={} observed={:?}",
        before_empty.len(),
        after_empty.len(),
        after_empty
    );
    assert!(after_empty.is_empty());

    let single = vec![(b"single-byte".to_vec(), b"z".to_vec())];
    db.put_batch(cf::CF_KV, single.clone())?;
    db.flush()?;
    let after_single = db.scan_cf(cf::CF_KV)?;
    println!(
        "regression_state=cf_scan edge=single_byte after_count={} observed={:?}",
        after_single.len(),
        printable_rows(&after_single)
    );
    assert_eq!(sorted_rows(after_single), sorted_rows(single));

    let boundary_value = vec![b'a'; batch::FLUSH_BYTES + 1];
    let boundary = vec![(b"size-boundary".to_vec(), boundary_value)];
    db.put_batch(cf::CF_EVENTS, boundary.clone())?;
    let after_boundary = db.scan_cf(cf::CF_EVENTS)?;
    println!(
        "regression_state=cf_scan edge=size_boundary after_count={} after_bytes={} observed=key:{:?} value_len:{}",
        after_boundary.len(),
        total_value_bytes(&after_boundary),
        String::from_utf8_lossy(&boundary[0].0),
        boundary[0].1.len()
    );
    assert_eq!(sorted_rows(after_boundary), sorted_rows(boundary));
    Ok(())
}

#[test]
fn must_pass_scan_cf_tail_returns_bounded_ascending_key_tail() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let rows = event_rows(8);
    db.put_batch(cf::CF_EVENTS, rows.clone())?;
    db.flush()?;

    let tail = db.scan_cf_tail(cf::CF_EVENTS, 3)?;
    println!(
        "regression_state=cf_tail case=bounded expected={:?} observed={:?}",
        printable_rows(&rows[5..]),
        printable_rows(&tail)
    );
    assert_eq!(tail, rows[5..].to_vec());
    assert!(db.scan_cf_tail(cf::CF_EVENTS, 0)?.is_empty());
    Ok(())
}

#[test]
fn must_pass_mutate_batch_pressure_bypass_deletes_and_puts_atomically() -> Result<(), Box<dyn Error>>
{
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    db.put_batch_pressure_bypass(
        cf::CF_SESSIONS,
        [
            (b"lease/owner-a".to_vec(), br#"{"session":"a"}"#.to_vec()),
            (b"lease/neighbor".to_vec(), br#"{"session":"n"}"#.to_vec()),
        ],
    )?;

    let before = db.scan_cf(cf::CF_SESSIONS)?;
    println!(
        "regression_state=cf_scan case=mutate_batch before_count={} observed={:?}",
        before.len(),
        printable_rows(&before)
    );
    db.mutate_batch_pressure_bypass(
        cf::CF_SESSIONS,
        [b"lease/owner-a".to_vec()],
        [(b"lease/owner-b".to_vec(), br#"{"session":"b"}"#.to_vec())],
    )?;

    let after = db.scan_cf(cf::CF_SESSIONS)?;
    println!(
        "regression_state=cf_scan case=mutate_batch after_count={} observed={:?}",
        after.len(),
        printable_rows(&after)
    );
    assert_eq!(
        sorted_rows(after),
        vec![
            (b"lease/neighbor".to_vec(), br#"{"session":"n"}"#.to_vec()),
            (b"lease/owner-b".to_vec(), br#"{"session":"b"}"#.to_vec()),
        ],
        "delete of owner-a and put of owner-b applied together; neighbor untouched"
    );
    Ok(())
}

#[test]
fn must_fail_acknowledged_put_cannot_resurrect_after_direct_delete() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let key = b"issue-44-delete".to_vec();
    {
        let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
        db.put_batch(cf::CF_KV, [(key.clone(), b"old".to_vec())])?;
        db.delete_batch(cf::CF_KV, [key.clone()])?;
        assert!(!row_exists(&db.scan_cf(cf::CF_KV)?, &key));
        std::thread::sleep(batch::FLUSH_INTERVAL.saturating_mul(2));
        assert!(!row_exists(&db.scan_cf(cf::CF_KV)?, &key));
        db.flush()?;
        assert!(!row_exists(&db.scan_cf(cf::CF_KV)?, &key));
    }
    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    assert!(!row_exists(&reopened.scan_cf(cf::CF_KV)?, &key));
    Ok(())
}

#[test]
fn must_fail_acknowledged_put_cannot_overwrite_newer_direct_replacement()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let key = b"issue-44-replace".to_vec();
    {
        let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
        db.put_batch(cf::CF_KV, [(key.clone(), b"old".to_vec())])?;
        db.mutate_batch_pressure_bypass(
            cf::CF_KV,
            [key.clone()],
            [(key.clone(), b"new".to_vec())],
        )?;
        assert_eq!(
            row_value(&db.scan_cf(cf::CF_KV)?, &key),
            Some(b"new".as_slice())
        );
        std::thread::sleep(batch::FLUSH_INTERVAL.saturating_mul(2));
        assert_eq!(
            row_value(&db.scan_cf(cf::CF_KV)?, &key),
            Some(b"new".as_slice())
        );
        db.flush()?;
        assert_eq!(
            row_value(&db.scan_cf(cf::CF_KV)?, &key),
            Some(b"new".as_slice())
        );
    }
    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    assert_eq!(
        row_value(&reopened.scan_cf(cf::CF_KV)?, &key),
        Some(b"new".as_slice())
    );
    Ok(())
}

fn event_rows(count: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    (0..count)
        .map(|index| {
            (
                format!("{index:016x}").into_bytes(),
                format!(r#"{{"ts_ns":{index},"event":"synthetic"}}"#).into_bytes(),
            )
        })
        .collect()
}

fn sorted_rows(mut rows: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<(Vec<u8>, Vec<u8>)> {
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    rows
}

fn total_value_bytes(rows: &[(Vec<u8>, Vec<u8>)]) -> usize {
    rows.iter().map(|(_key, value)| value.len()).sum()
}

fn row_exists(rows: &[(Vec<u8>, Vec<u8>)], key: &[u8]) -> bool {
    rows.iter().any(|(candidate, _value)| candidate == key)
}

fn row_value<'a>(rows: &'a [(Vec<u8>, Vec<u8>)], key: &[u8]) -> Option<&'a [u8]> {
    rows.iter()
        .find_map(|(candidate, value)| (candidate == key).then_some(value.as_slice()))
}

fn printable_rows(rows: &[(Vec<u8>, Vec<u8>)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| {
            (
                String::from_utf8_lossy(key).into_owned(),
                String::from_utf8_lossy(value).into_owned(),
            )
        })
        .collect()
}
