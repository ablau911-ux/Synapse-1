use super::*;
use std::{error::Error, fs};

use rocksdb::DEFAULT_COLUMN_FAMILY_NAME;
use synapse_core::error_codes;

const TEST_SCHEMA_VERSION: u32 = 7;
const DURABILITY_KEY: &[u8] = b"restart-durability-key";
const DURABILITY_VALUE: &[u8] = b"restart-durability-value";

#[test]
fn must_pass_open_fresh_db_creates_all_prd_cfs_and_schema_sentinel() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    println!(
        "regression_state=db.list_cf edge=fresh before_path={} before={:?}",
        path.display(),
        list_cf_for(&path)
    );

    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let handles = existing_prd_handles(&db);
    let sentinel = db.raw()?.get(SCHEMA_VERSION_KEY)?;
    let expected_schema = TEST_SCHEMA_VERSION.to_be_bytes();
    println!(
        "regression_state=db.list_cf edge=fresh after_handles={handles:?} after_sentinel={:?} observed=count:{}",
        sentinel.as_deref(),
        handles.len()
    );
    assert_eq!(handles, sorted_prd_cfs());
    assert_eq!(sentinel.as_deref(), Some(expected_schema.as_slice()));
    drop(db);

    let physical = sorted_list_cf(&path)?;
    println!("regression_state=db.list_cf edge=fresh physical_after_drop={physical:?}");
    assert_eq!(physical, sorted_physical_cfs());
    Ok(())
}

#[test]
fn must_pass_opens_database_created_with_every_pre_cf_layout() -> Result<(), Box<dyn Error>> {
    // Every historical layout an older binary could leave on disk: CFs were
    // added in this order (pre-ADR timeline, #846, #848, #849, #897, #900), so
    // a legacy DB lacks a suffix of this list. Opening must add the missing
    // CFs, keep the newest pre-existing CF's row, and accept a write to the
    // first added CF.
    let added_in_order = [
        cf::CF_TIMELINE,
        cf::CF_EPISODES,
        cf::CF_ROUTINES,
        cf::CF_ROUTINE_STATE,
        cf::CF_AGENT_EVENTS,
        cf::CF_AGENT_TRANSCRIPTS,
    ];
    for first_missing in 0..added_in_order.len() {
        let missing = &added_in_order[first_missing..];
        let preserved_cf = first_missing
            .checked_sub(1)
            .map_or(cf::CF_EVENTS, |index| added_in_order[index]);
        assert_additive_open(missing, preserved_cf, legacy_new_row(missing[0]))?;
    }
    Ok(())
}

fn legacy_new_row(cf_name: &str) -> (Vec<u8>, Vec<u8>) {
    if cf_name == cf::CF_AGENT_EVENTS {
        (
            agent_events::agent_event_key(1, 0),
            br#"{"ts_ns":1}"#.to_vec(),
        )
    } else if cf_name == cf::CF_AGENT_TRANSCRIPTS {
        (
            agent_transcripts::agent_transcript_key("agent-spawn-migrate", 1),
            br#"{"ts_ns":1}"#.to_vec(),
        )
    } else if cf_name == cf::CF_ROUTINES || cf_name == cf::CF_ROUTINE_STATE {
        (b"rt1-0123456789abcdef".to_vec(), b"{}".to_vec())
    } else {
        (b"migrate".to_vec(), b"{}".to_vec())
    }
}

fn assert_additive_open(
    missing: &[&str],
    preserved_cf: &str,
    row: (Vec<u8>, Vec<u8>),
) -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let edge = missing[0];
    let legacy_cfs: Vec<&str> = cf::ALL_COLUMN_FAMILIES
        .into_iter()
        .filter(|name| !missing.contains(name))
        .collect();
    {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        let legacy = DB::open_cf(&options, &path, &legacy_cfs)?;
        legacy.put(SCHEMA_VERSION_KEY, TEST_SCHEMA_VERSION.to_be_bytes())?;
        let handle = legacy
            .cf_handle(preserved_cf)
            .ok_or("legacy preserved CF handle missing")?;
        legacy.put_cf(&handle, DURABILITY_KEY, DURABILITY_VALUE)?;
        legacy.flush_cf(&handle)?;
    }
    let before = sorted_list_cf(&path)?;
    println!("regression_state=db.list_cf edge=pre_{edge} before={before:?}");
    for name in missing {
        assert!(
            !before.contains(&(*name).to_owned()),
            "precondition: legacy layout must not contain {name}"
        );
    }

    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let handles = existing_prd_handles(&db);
    let raw = db.raw()?;
    let preserved = raw
        .cf_handle(preserved_cf)
        .and_then(|handle| raw.get_cf(&handle, DURABILITY_KEY).transpose())
        .transpose()?;
    db.put_batch(edge, vec![row.clone()])?;
    db.flush()?;
    let new_rows = db.scan_cf(edge)?;
    drop(raw);
    drop(db);

    let after = sorted_list_cf(&path)?;
    println!("regression_state=db.list_cf edge=pre_{edge} after={after:?} preserved={preserved:?}");
    assert_eq!(handles, sorted_prd_cfs(), "{edge}");
    assert_eq!(preserved.as_deref(), Some(DURABILITY_VALUE), "{edge}");
    assert_eq!(new_rows, vec![row], "{edge}");
    assert_eq!(after, sorted_physical_cfs(), "{edge}");
    Ok(())
}

#[test]
fn must_pass_opens_database_holding_a_future_unknown_cf() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");

    // A newer binary created the database, added a CF this binary does not
    // know, and wrote rows into it (the 2026-06-12 rollback incident shape:
    // a pre-CF_ROUTINES daemon died at startup on a post-CF_ROUTINES DB).
    {
        let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
        db.put_batch(
            cf::CF_EVENTS,
            vec![(DURABILITY_KEY.to_vec(), DURABILITY_VALUE.to_vec())],
        )?;
        db.flush()?;
    }
    {
        let mut options = Options::default();
        options.create_if_missing(false);
        options.create_missing_column_families(true);
        let mut all: Vec<&str> = cf::ALL_COLUMN_FAMILIES.to_vec();
        all.push("CF_FROM_THE_FUTURE");
        let future = DB::open_cf(&options, &path, &all)?;
        let handle = future
            .cf_handle("CF_FROM_THE_FUTURE")
            .ok_or("future CF handle missing")?;
        future.put_cf(&handle, b"future-key", b"future-value")?;
        future.flush_cf(&handle)?;
    }
    let before = sorted_list_cf(&path)?;
    println!("regression_state=db.list_cf edge=future_cf before={before:?}");
    assert!(before.contains(&"CF_FROM_THE_FUTURE".to_owned()));

    // This binary must open the database anyway, preserve both its own rows
    // and the unknown CF's rows, and leave the unknown CF physically intact.
    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let raw = db.raw()?;
    let preserved = raw
        .cf_handle(cf::CF_EVENTS)
        .and_then(|handle| raw.get_cf(&handle, DURABILITY_KEY).transpose())
        .transpose()?;
    let future_value = raw
        .cf_handle("CF_FROM_THE_FUTURE")
        .and_then(|handle| raw.get_cf(&handle, b"future-key").transpose())
        .transpose()?;
    assert_eq!(preserved.as_deref(), Some(DURABILITY_VALUE));
    assert_eq!(future_value.as_deref(), Some(b"future-value".as_slice()));
    drop(raw);
    drop(db);

    let after = sorted_list_cf(&path)?;
    println!(
        "regression_state=db.list_cf edge=future_cf after={after:?} observed=tolerant_open:ok"
    );
    assert!(after.contains(&"CF_FROM_THE_FUTURE".to_owned()));
    Ok(())
}

#[test]
fn must_fail_mismatched_schema_errors_then_wipe_retry_succeeds() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Db::open(&path, 1)?;
    let before = db.raw()?.get(SCHEMA_VERSION_KEY)?;
    drop(db);
    println!(
        "regression_state=db.schema edge=mismatch before_path={} before_sentinel={before:?}",
        path.display()
    );

    let error = match Db::open(&path, TEST_SCHEMA_VERSION) {
        Ok(db) => panic!("Db::open unexpectedly accepted mismatched schema: {db:?}"),
        Err(error) => error,
    };
    println!(
        "regression_state=db.schema edge=mismatch after_code={} after_db_exists={}",
        error.code(),
        path.exists()
    );
    assert_eq!(error.code(), error_codes::STORAGE_SCHEMA_MISMATCH);
    let old_schema = 1_u32.to_be_bytes();
    assert_eq!(before.as_deref(), Some(old_schema.as_slice()));
    // The refused open must not have rewritten the on-disk sentinel.
    let refused_sentinel = {
        let read_only = DB::open_for_read_only(&Options::default(), &path, false)?;
        read_only.get(SCHEMA_VERSION_KEY)?
    };
    assert_eq!(
        refused_sentinel.as_deref(),
        Some(old_schema.as_slice()),
        "refused open must leave the old schema sentinel untouched"
    );

    fs::remove_dir_all(&path)?;
    println!(
        "regression_state=db.schema edge=mismatch after_wipe_exists={}",
        path.exists()
    );
    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let after = db.raw()?.get(SCHEMA_VERSION_KEY)?;
    let expected_schema = TEST_SCHEMA_VERSION.to_be_bytes();
    println!(
        "regression_state=db.schema edge=mismatch retry_sentinel={after:?} observed=schema_version:{}",
        db.schema_version
    );
    assert_eq!(after.as_deref(), Some(expected_schema.as_slice()));
    Ok(())
}

#[test]
fn must_pass_process_restart_reads_persisted_key() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let before = db.scan_cf(cf::CF_KV)?;
    println!(
        "regression_state=db.restart edge=durability before_key={:?} before_rows={}",
        String::from_utf8_lossy(DURABILITY_KEY),
        before.len()
    );
    assert!(before.is_empty());
    // Production write path: the batcher, then an explicit flush.
    db.put_batch(
        cf::CF_KV,
        vec![(DURABILITY_KEY.to_vec(), DURABILITY_VALUE.to_vec())],
    )?;
    db.flush()?;
    drop(db);

    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let after = db.scan_cf(cf::CF_KV)?;
    println!(
        "regression_state=db.restart edge=durability after_rows={} observed={:?}",
        after.len(),
        Some(DURABILITY_VALUE)
    );
    assert_eq!(
        after,
        vec![(DURABILITY_KEY.to_vec(), DURABILITY_VALUE.to_vec())]
    );
    Ok(())
}

#[test]
fn must_fail_file_path_open_fails_with_storage_open_failed() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db-file");
    fs::write(&path, b"not a directory")?;
    println!(
        "regression_state=db.list_cf edge=file_path before_path={} before_is_file={}",
        path.display(),
        path.is_file()
    );
    let error = match Db::open(&path, TEST_SCHEMA_VERSION) {
        Ok(db) => panic!("Db::open unexpectedly accepted file path: {db:?}"),
        Err(error) => error,
    };
    println!(
        "regression_state=db.list_cf edge=file_path after_code={} after_detail={:?} after_is_file={}",
        error.code(),
        open_error_detail(&error),
        path.is_file()
    );
    assert_eq!(error.code(), error_codes::STORAGE_OPEN_FAILED);
    Ok(())
}

#[cfg(windows)]
#[test]
fn must_fail_non_writable_path_returns_storage_open_failed() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("locked-db");
    fs::create_dir_all(&path)?;
    let guard = deny_current_user_write(&path)?;
    println!(
        "regression_state=db.list_cf edge=permission before_path={} before={:?}",
        path.display(),
        list_cf_for(&path)
    );

    let error = match Db::open(&path, TEST_SCHEMA_VERSION) {
        Ok(db) => panic!("Db::open unexpectedly accepted non-writable path: {db:?}"),
        Err(error) => error,
    };
    println!(
        "regression_state=db.list_cf edge=permission after_code={} after_detail={:?} after_exists={}",
        error.code(),
        open_error_detail(&error),
        path.exists()
    );
    // Restore the ACL before any further filesystem access; nothing after
    // this point uses `?`, so teardown can never fail the test.
    drop(guard);
    assert_eq!(error.code(), error_codes::STORAGE_OPEN_FAILED);
    // Locale-independent: RocksDB's Windows message text ("Access is denied")
    // is translated per OS UI language and carries no numeric code.
    assert!(!open_error_detail(&error).is_empty());
    // No usable database was produced: no RocksDB CURRENT manifest pointer.
    // (The directory itself may remain; that is not part of the contract.)
    assert!(
        !path.join("CURRENT").exists(),
        "refused open must not leave a usable database behind"
    );
    Ok(())
}

fn existing_prd_handles(db: &Db) -> Vec<&'static str> {
    let mut names = cf::ALL_COLUMN_FAMILIES
        .into_iter()
        .filter(|name| db.raw().is_ok_and(|raw| raw.cf_handle(name).is_some()))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn sorted_prd_cfs() -> Vec<&'static str> {
    let mut names = cf::ALL_COLUMN_FAMILIES.to_vec();
    names.sort_unstable();
    names
}

fn sorted_physical_cfs() -> Vec<String> {
    let mut names = sorted_prd_cfs()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.push(DEFAULT_COLUMN_FAMILY_NAME.to_owned());
    names.sort_unstable();
    names
}

fn sorted_list_cf(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut cfs = DB::list_cf(&Options::default(), path)?;
    cfs.sort_unstable();
    Ok(cfs)
}

fn list_cf_for(path: &Path) -> Result<Vec<String>, String> {
    sorted_list_cf(path).map_err(|error| error.to_string())
}

fn open_error_detail(error: &StorageError) -> &str {
    match error {
        StorageError::OpenFailed { detail, .. } => detail,
        _ => "",
    }
}

#[cfg(windows)]
pub(crate) struct DenyWriteGuard {
    path: std::path::PathBuf,
    principal: String,
}

#[cfg(windows)]
impl Drop for DenyWriteGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(&self.path)
            .args(["/remove:d", &self.principal])
            .output();
    }
}

#[cfg(windows)]
pub(crate) fn deny_current_user_write(path: &Path) -> Result<DenyWriteGuard, Box<dyn Error>> {
    let domain = std::env::var("USERDOMAIN")?;
    let username = std::env::var("USERNAME")?;
    let principal = format!(r"{domain}\{username}");
    let deny = format!("{principal}:(W)");
    let output = std::process::Command::new("icacls")
        .arg(path)
        .args(["/deny", &deny])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "icacls deny failed: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(DenyWriteGuard {
        path: path.to_path_buf(),
        principal,
    })
}
