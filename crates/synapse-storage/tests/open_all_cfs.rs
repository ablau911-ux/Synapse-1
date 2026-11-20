use std::error::Error;

use synapse_storage::{Db, cf};

const TEST_SCHEMA_VERSION: u32 = 7;

#[test]
fn must_pass_open_all_cfs_and_restart_durability() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    println!(
        "regression_state=open_all_cfs before_exists={} after=not_opened observed=path:{}",
        path.exists(),
        path.display()
    );

    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    for cf_name in cf::ALL_COLUMN_FAMILIES {
        let before = db.scan_cf(cf_name)?;
        db.put_batch(cf_name, row(cf_name))?;
        println!(
            "regression_state=open_all_cfs cf={cf_name} before={} after=enqueued observed=key:{}",
            before.len(),
            key(cf_name)
        );
    }
    db.flush()?;
    let after_counts = cf_counts(&db)?;
    println!(
        "regression_state=open_all_cfs after_open_counts={after_counts:?} after=all_cfs_writable observed=count:{}",
        after_counts.len()
    );
    assert!(after_counts.iter().all(|(_cf_name, count)| *count == 1));
    drop(db);

    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let reopened_counts = cf_counts(&reopened)?;
    println!(
        "regression_state=open_all_cfs after_reopen_counts={reopened_counts:?} after=durable observed=count:{}",
        reopened_counts.len()
    );
    assert!(reopened_counts.iter().all(|(_cf_name, count)| *count == 1));
    Ok(())
}

fn cf_counts(db: &Db) -> Result<Vec<(&'static str, usize)>, Box<dyn Error>> {
    cf::ALL_COLUMN_FAMILIES
        .into_iter()
        .map(|cf_name| Ok((cf_name, db.scan_cf(cf_name)?.len())))
        .collect()
}

fn row(cf_name: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![(
        key(cf_name).into_bytes(),
        format!(r#"{{"label":"{cf_name}"}}"#).into_bytes(),
    )]
}

fn key(cf_name: &str) -> String {
    format!("open-all-cfs-{cf_name}")
}
