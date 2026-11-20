//! Regression tests for #52: the storage diagnostics cannot plant or reveal
//! `CF_OCR_CACHE` row content.

use std::sync::{Arc, Mutex};

use synapse_action::ActionHandle;
use synapse_core::SCHEMA_VERSION;
use synapse_reflex::EventBus;
use synapse_storage::Db;
use tempfile::tempdir;

use super::*;

type TestRuntime = (tempfile::TempDir, Arc<Db>, Arc<Mutex<ReflexRuntime>>);

fn runtime_with_db() -> anyhow::Result<TestRuntime> {
    let temp = tempdir()?;
    let db = Arc::new(Db::open(&temp.path().join("db"), SCHEMA_VERSION)?);
    let (action, _rx) = ActionHandle::channel();
    let runtime = Arc::new(Mutex::new(ReflexRuntime::spawn(
        Arc::clone(&db),
        action,
        EventBus::default(),
    )?));
    Ok((temp, db, runtime))
}

fn error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn ocr_cache_rows(db: &Db) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    Ok(db.scan_cf_prefix(cf::CF_OCR_CACHE, b"")?)
}

fn ocr_cache_count(runtime: &Arc<Mutex<ReflexRuntime>>) -> anyhow::Result<u64> {
    let guard = runtime.lock().map_err(|_e| anyhow::anyhow!("lock"))?;
    let counts = guard
        .storage_cf_row_counts()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(counts.get(cf::CF_OCR_CACHE).copied().unwrap_or_default())
}

#[test]
fn must_fail_probe_writes_to_the_ocr_cache_are_refused() -> anyhow::Result<()> {
    let (_temp, db, runtime) = runtime_with_db()?;
    db.put_batch_pressure_bypass(
        cf::CF_OCR_CACHE,
        [(
            b"ocr/cache/v2/existing".to_vec(),
            b"{\"sealed_v\":1}".to_vec(),
        )],
    )?;
    let rows_before = ocr_cache_rows(&db)?;
    let count_before = ocr_cache_count(&runtime)?;

    let error = put_probe_rows(
        &runtime,
        &StoragePutProbeRowsParams {
            cf_name: "CF_OCR_CACHE".to_owned(),
            key_prefix: "probe-52".to_owned(),
            rows: 3,
            value_bytes: 16,
            value_json: None,
            ts_ns_start: None,
            ts_ns_step: None,
            key_mode: None,
        },
    )
    .err()
    .ok_or_else(|| anyhow::anyhow!("a probe write to CF_OCR_CACHE must be refused"))?;

    assert_eq!(error_code(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    assert!(
        error
            .message
            .contains("storage diagnostic writes support only"),
        "{error:?}"
    );
    assert_eq!(ocr_cache_count(&runtime)?, count_before);
    assert_eq!(
        ocr_cache_rows(&db)?,
        rows_before,
        "a refused probe must not plant rows"
    );
    Ok(())
}

#[test]
fn must_pass_every_other_column_family_stays_probe_writable() {
    for cf_name in cf::ALL_COLUMN_FAMILIES {
        if cf_name == cf::CF_OCR_CACHE {
            continue;
        }
        assert_eq!(probe_writable_cf(cf_name).ok(), Some(cf_name));
    }
    assert_eq!(PROBE_WRITABLE_CFS.len(), cf::ALL_COLUMN_FAMILIES.len() - 1);
}

fn gc_params(cf_name: &str) -> StorageGcOnceParams {
    StorageGcOnceParams {
        cf_name: cf_name.to_owned(),
        soft_cap_rows: 1,
        hard_cap_rows: 1,
        run_id: None,
        now_ns: None,
        max_age_ns: None,
        dedupe_window_ns: None,
        profile_id: None,
    }
}

#[test]
fn must_pass_gc_may_still_cap_the_ocr_cache() -> anyhow::Result<()> {
    assert_eq!(gc_cf("CF_OCR_CACHE").ok(), Some("CF_OCR_CACHE"));
    // Through the real storage_gc_once entry, not only the name check.
    let (_temp, _db, runtime) = runtime_with_db()?;
    let response = run_storage_gc_once(&runtime, &gc_params("CF_OCR_CACHE"))
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    assert_eq!(response.cf_name, "CF_OCR_CACHE");
    assert_eq!(response.before_rows, 0);
    assert_eq!(response.after_rows, 0);
    Ok(())
}

#[test]
fn must_fail_gc_rejects_unknown_cf() -> anyhow::Result<()> {
    let (_temp, _db, runtime) = runtime_with_db()?;
    let error = run_storage_gc_once(&runtime, &gc_params("CF_NOT_A_REAL_CF"))
        .err()
        .ok_or_else(|| anyhow::anyhow!("an unknown column family must be refused"))?;
    assert_eq!(error_code(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    assert!(
        error.message.contains("got \"CF_NOT_A_REAL_CF\""),
        "{error:?}"
    );
    Ok(())
}
