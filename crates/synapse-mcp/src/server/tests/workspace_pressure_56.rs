use super::*;
use crate::server::g7b_support::{code, now_ns, service};
use synapse_storage::DiskPressureLevel;

fn put(key: &str) -> WorkspacePutParams {
    WorkspacePutParams {
        run_id: Some("g7b-pressure".into()),
        key: key.into(),
        expected_version: None,
        value: Some(json!({"exact": 56})),
        artifact: None,
        ttl_ms: 600_000,
    }
}

#[test]
fn must_pass_workspace_put_persists_exact_value_56() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.workspace_db()?;
    let key = workspace_row_key("g7b-pressure", "allowed");
    assert_eq!(db.get_cf_exact(cf::CF_KV, key.as_bytes())?, None);
    let response = svc.workspace_put_impl_at(put("allowed"), "g7b", now_ns() / 1_000_000)?;
    assert_eq!(response.version, 1);
    let stored =
        read_workspace_row_optional(&db, &key, now_ns() / 1_000_000)?.expect("physical row");
    assert_eq!(stored.entry.value, Some(json!({"exact": 56})));
    assert_eq!(stored.entry.version, 1);
    Ok(())
}

#[test]
fn must_fail_pressure_refused_workspace_put_has_no_kv_side_effect_56() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.workspace_db()?;
    db.run_pressure_check_with_free_bytes_sample(100_000_000)?;
    assert_eq!(db.pressure_level(), DiskPressureLevel::Level4);
    assert!(!db.pressure_permits_write(cf::CF_KV));
    let before = db.scan_cf(cf::CF_KV)?;
    let error = svc
        .workspace_put_impl_at(put("refused"), "g7b", now_ns() / 1_000_000)
        .expect_err("pressure must refuse ordinary workspace ingestion");
    assert_eq!(code(&error), "STORAGE_WRITE_SHED");
    assert_eq!(db.scan_cf(cf::CF_KV)?, before);
    Ok(())
}
