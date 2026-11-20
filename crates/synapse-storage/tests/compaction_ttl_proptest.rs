use std::{
    error::Error,
    time::{SystemTime, UNIX_EPOCH},
};

use synapse_storage::{Db, cf};

const TEST_SCHEMA_VERSION: u32 = 7;
const HOUR_NS: u64 = 3_600_000_000_000;
const DAY_NS: u64 = 24 * HOUR_NS;
// Literal compaction TTL per CF from the retention spec, independent of the
// DEFAULTS table and exemption list the code reads. `None` = rows never expire
// by compaction: no TTL (PROFILES, KV, ROUTINES, ROUTINE_STATE), LRU-only
// (MODEL_CACHE), or exempt live state without a root `ts_ns` (SESSIONS, #43).
const EXPECTED_COMPACTION_TTL_NS: [(&str, Option<u64>); 17] = [
    ("CF_EVENTS", Some(24 * HOUR_NS)),
    ("CF_OBSERVATIONS", Some(6 * HOUR_NS)),
    ("CF_PROFILES", None),
    ("CF_MODEL_CACHE", None),
    ("CF_SESSIONS", None),
    ("CF_REFLEX_AUDIT", Some(7 * DAY_NS)),
    ("CF_OCR_CACHE", Some(HOUR_NS)),
    ("CF_TELEMETRY", Some(6 * HOUR_NS)),
    ("CF_ACTION_LOG", Some(24 * HOUR_NS)),
    ("CF_PROCESS_HISTORY", Some(6 * HOUR_NS)),
    ("CF_KV", None),
    ("CF_TIMELINE", Some(90 * DAY_NS)),
    ("CF_EPISODES", Some(90 * DAY_NS)),
    ("CF_ROUTINES", None),
    ("CF_ROUTINE_STATE", None),
    ("CF_AGENT_EVENTS", Some(30 * DAY_NS)),
    ("CF_AGENT_TRANSCRIPTS", Some(30 * DAY_NS)),
];

#[test]
fn must_fail_compaction_ttl_old_fresh_invalid_and_restart() -> Result<(), Box<dyn Error>> {
    let mut table_cfs: Vec<&str> = EXPECTED_COMPACTION_TTL_NS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let mut declared = cf::ALL_COLUMN_FAMILIES.to_vec();
    table_cfs.sort_unstable();
    declared.sort_unstable();
    assert_eq!(
        table_cfs, declared,
        "expected-TTL table must cover every CF"
    );

    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let now_ns = now_ns()?;

    for (cf_name, ttl_ns) in EXPECTED_COMPACTION_TTL_NS {
        db.put_batch(
            cf_name,
            vec![
                row("old", 0),
                row("fresh", now_ns),
                (b"invalid".to_vec(), br#"{"label":"invalid"}"#.to_vec()),
            ],
        )?;
        db.flush()?;
        db.compact_cf(cf_name)?;
        let after = db.scan_cf(cf_name)?;
        let old_after = count_key(&after, "old");
        println!(
            "regression_state=ttl_cf_scan cf={cf_name} after=old:{old_after},fresh:{},invalid:{} observed=ttl_ns:{ttl_ns:?}",
            count_key(&after, "fresh"),
            count_key(&after, "invalid")
        );
        // Epoch-0 row is older than any TTL: expired iff the CF has one.
        assert_eq!(old_after, usize::from(ttl_ns.is_none()), "{cf_name} old");
        assert_eq!(count_key(&after, "fresh"), 1, "{cf_name} fresh");
        assert_eq!(count_key(&after, "invalid"), 1, "{cf_name} no ts_ns");
        assert_eq!(after.len(), expected_rows(ttl_ns), "{cf_name}");
    }
    drop(db);

    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    for (cf_name, ttl_ns) in EXPECTED_COMPACTION_TTL_NS {
        let rows = reopened.scan_cf(cf_name)?;
        println!(
            "regression_state=ttl_cf_scan cf={cf_name} edge=restart after=count:{} observed=durable:true",
            rows.len()
        );
        assert_eq!(rows.len(), expected_rows(ttl_ns), "{cf_name} after restart");
    }
    Ok(())
}

/// fresh + invalid always survive; the epoch-0 row survives only without a TTL.
fn expected_rows(ttl_ns: Option<u64>) -> usize {
    if ttl_ns.is_some() { 2 } else { 3 }
}

#[test]
fn must_pass_compaction_ttl_keeps_rows_inside_the_ttl_byte_for_byte() -> Result<(), Box<dyn Error>>
{
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let now_ns = now_ns()?;
    let mut checked = 0_usize;

    for (cf_name, ttl_ns) in EXPECTED_COMPACTION_TTL_NS {
        let Some(ttl_ns) = ttl_ns else {
            continue;
        };
        // Age == ttl - 60 s is still inside the TTL (removal needs age > ttl).
        let edge_ts = now_ns.saturating_sub(ttl_ns.saturating_sub(60_000_000_000));
        let half_ts = now_ns.saturating_sub(ttl_ns / 2);
        let written = vec![row("edge", edge_ts), row("half", half_ts)];
        db.put_batch(cf_name, written.clone())?;
        db.flush()?;
        db.compact_cf(cf_name)?;
        let after = db.scan_cf(cf_name)?;
        println!(
            "regression_state=ttl_cf_scan cf={cf_name} edge=inside_ttl after=count:{} observed=ttl_ns:{ttl_ns}",
            after.len()
        );
        for (key, value) in &written {
            let kept = after
                .iter()
                .find(|(row_key, _value)| row_key == key)
                .map(|(_key, row_value)| row_value);
            assert_eq!(
                kept,
                Some(value),
                "{cf_name}: row {:?} inside the TTL must survive compaction unchanged",
                String::from_utf8_lossy(key)
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 11, "every TTL-bearing CF must be checked");
    Ok(())
}

fn row(label: &str, ts_ns: u64) -> (Vec<u8>, Vec<u8>) {
    (
        label.as_bytes().to_vec(),
        format!(r#"{{"ts_ns":{ts_ns},"label":"{label}"}}"#).into_bytes(),
    )
}

fn count_key(rows: &[(Vec<u8>, Vec<u8>)], key: &str) -> usize {
    rows.iter()
        .filter(|(row_key, _value)| row_key == key.as_bytes())
        .count()
}

fn now_ns() -> Result<u64, Box<dyn Error>> {
    Ok(u64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
    )?)
}
