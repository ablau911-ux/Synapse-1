use std::error::Error;

use synapse_core::error_codes;
use synapse_storage::{Db, cf};

const TEST_SCHEMA_VERSION: u32 = 7;

#[test]
fn must_fail_gc_soft_cap_edges_and_restart() -> Result<(), Box<dyn Error>> {
    let cases = [
        Case::new("below_soft", 9, 10, 20, 9, 0, None),
        Case::new("at_soft", 10, 10, 20, 10, 0, None),
        Case::new("soft_cap", 20, 10, 30, 10, 10, None),
        // Small trim: capping 100 rows at 96 must evict EXACTLY the 4-row overage
        // and land on 96 — explicit row caps have no minimum batch. (The retired
        // 25%-floor would have evicted 25 here and left 75; this case guards the
        // operator "cap at N" semantics the dashboard storage manager relies on.)
        Case::new("small_trim_no_quarter_floor", 100, 96, 200, 96, 4, None),
        Case::new(
            "hard_cap",
            25,
            10,
            20,
            10,
            15,
            Some(error_codes::STORAGE_CF_HARD_CAP_REACHED),
        ),
    ];

    for case in cases {
        run_case(case)?;
    }
    Ok(())
}

#[test]
fn must_pass_gc_soft_cap_evicts_exactly_the_oldest_rows_and_keeps_the_newest_intact()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let rows = event_rows(20);
    db.put_batch(cf::CF_EVENTS, rows.clone())?;
    db.flush()?;

    let report = db.run_gc_once_with_row_caps(cf::CF_EVENTS, 10, 30)?;
    let cf_report = report.cf(cf::CF_EVENTS).ok_or("missing CF_EVENTS report")?;
    assert_eq!(cf_report.before_value, 20);
    assert_eq!(cf_report.after_value, 10);
    assert_eq!(cf_report.evicted_rows, 10);
    assert!(!cf_report.hard_cap_reached);
    assert_eq!(cf_report.hard_cap_code, None);

    let after = db.scan_cf(cf::CF_EVENTS)?;
    println!(
        "regression_state=gc_cf_scan case=oldest_first after=count:{} observed=keys:{:?}",
        after.len(),
        printable_keys(&after)
    );
    // Keys sort by age, so the 10 newest rows survive with unchanged values.
    assert_eq!(after, rows[10..].to_vec());
    drop(db);

    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    assert_eq!(reopened.scan_cf(cf::CF_EVENTS)?, rows[10..].to_vec());
    Ok(())
}

fn run_case(case: Case) -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    db.put_batch(cf::CF_EVENTS, event_rows(case.rows))?;
    db.flush()?;
    let before = db.scan_cf(cf::CF_EVENTS)?;
    let report = db.run_gc_once_with_row_caps(cf::CF_EVENTS, case.soft_cap, case.hard_cap)?;
    let cf_report = report.cf(cf::CF_EVENTS).ok_or("missing CF_EVENTS report")?;
    let after = db.scan_cf(cf::CF_EVENTS)?;
    println!(
        "regression_state=gc_cf_scan case={} before={} after=count:{} evicted:{} hard_cap_code:{:?} observed=keys:{:?}",
        case.name,
        before.len(),
        after.len(),
        cf_report.evicted_rows,
        cf_report.hard_cap_code,
        printable_keys(&after)
    );
    assert_eq!(after.len(), case.expected_after);
    assert_eq!(cf_report.evicted_rows, case.expected_evicted);
    assert_eq!(cf_report.hard_cap_code, case.expected_code);
    drop(db);

    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let reopened_rows = reopened.scan_cf(cf::CF_EVENTS)?;
    println!(
        "regression_state=gc_cf_scan case={} edge=restart before=dropped after=count:{} observed=durable:{}",
        case.name,
        reopened_rows.len(),
        reopened_rows.len() == case.expected_after
    );
    assert_eq!(reopened_rows.len(), case.expected_after);
    Ok(())
}

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    rows: usize,
    soft_cap: u64,
    hard_cap: u64,
    expected_after: usize,
    expected_evicted: u64,
    expected_code: Option<&'static str>,
}

impl Case {
    const fn new(
        name: &'static str,
        rows: usize,
        soft_cap: u64,
        hard_cap: u64,
        expected_after: usize,
        expected_evicted: u64,
        expected_code: Option<&'static str>,
    ) -> Self {
        Self {
            name,
            rows,
            soft_cap,
            hard_cap,
            expected_after,
            expected_evicted,
            expected_code,
        }
    }
}

fn event_rows(count: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    (0..count)
        .map(|index| {
            (
                format!("{index:016x}").into_bytes(),
                format!(r#"{{"label":"gc","seq":{index}}}"#).into_bytes(),
            )
        })
        .collect()
}

fn printable_keys(rows: &[(Vec<u8>, Vec<u8>)]) -> Vec<String> {
    rows.iter()
        .map(|(key, _value)| String::from_utf8_lossy(key).into_owned())
        .collect()
}
