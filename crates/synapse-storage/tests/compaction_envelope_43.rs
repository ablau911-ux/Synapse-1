//! Exercise the installed RocksDB filter, rather than test-only ttl_decision.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use synapse_storage::{Db, cf};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
        .try_into()
        .expect("fits")
}

fn key(ts: u64, seq: u32) -> Vec<u8> {
    [ts.to_be_bytes().as_slice(), seq.to_be_bytes().as_slice()].concat()
}

#[test]
fn must_pass_live_envelopes_keep_exact_bytes_despite_nested_epoch_43() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let db = Db::open(dir.path(), synapse_core::SCHEMA_VERSION)?;
    let ts = now();
    for name in [cf::CF_OBSERVATIONS, cf::CF_TIMELINE] {
        let rows = vec![
            (
                key(ts, 1),
                format!(r#"{{"payload":{{"ts_ns":0}},"ts_ns":{ts}}}"#).into_bytes(),
            ),
            (
                key(ts, 2),
                format!(r#"{{"title":"\\\"ts_ns\\\":0","ts_ns":{ts}}}"#).into_bytes(),
            ),
            (key(ts, 3), br#"{"payload":{"ts_ns":0}}"#.to_vec()),
            (
                key(ts, 4),
                br#"{"ts_ns":"invalid","payload":{"ts_ns":0}}"#.to_vec(),
            ),
        ];
        db.put_batch(name, rows.clone())?;
        assert_eq!(db.scan_cf(name)?, rows);
        db.flush()?;
        db.compact_cf(name)?;
        assert_eq!(
            db.scan_cf(name)?,
            rows,
            "{name}: nested input cannot evict fresh envelope"
        );
    }
    Ok(())
}

#[test]
fn must_fail_nested_fresh_timestamp_cannot_resurrect_expired_key_43() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let db = Db::open(dir.path(), synapse_core::SCHEMA_VERSION)?;
    let ts = now();
    for name in [cf::CF_OBSERVATIONS, cf::CF_TIMELINE] {
        let rows = vec![
            (
                key(1, 1),
                format!(r#"{{"payload":{{"ts_ns":{ts}}}}}"#).into_bytes(),
            ),
            (
                key(1, 2),
                format!(r#"{{"payload":{{"ts_ns":{ts}}},"ts_ns":0}}"#).into_bytes(),
            ),
            (key(1, 3), br#"{"ts_ns":"invalid"}"#.to_vec()),
        ];
        db.put_batch(name, rows.clone())?;
        assert_eq!(db.scan_cf(name)?, rows);
        db.flush()?;
        db.compact_cf(name)?;
        assert_eq!(db.scan_cf(name)?, Vec::<(Vec<u8>, Vec<u8>)>::new());
    }
    Ok(())
}
