//! #56: a write whose explicit WAL flush fails must never be acknowledged.
//! The unsynced row can exist: refusal is not a transactional rollback promise.
use super::*;
use crate::server::g7b_support::{code, now_ns, service};

fn params(key: &str) -> WorkspacePutParams {
    WorkspacePutParams {
        run_id: Some("fu2-flush".into()),
        key: key.into(),
        expected_version: None,
        value: Some(json!({"exact":56})),
        artifact: None,
        ttl_ms: 600_000,
    }
}

#[test]
fn must_fail_workspace_put_does_not_acknowledge_failed_flush() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.workspace_db()?;
    let now = now_ns() / 1_000_000;
    let key = workspace_row_key("fu2-flush", "failed");
    assert_eq!(db.get_cf_exact(cf::CF_KV, key.as_bytes())?, None);
    db.put_batch(
        cf::CF_KV,
        [(b"fu2-neighbor".to_vec(), b"unchanged".to_vec())],
    )?;
    db.flush()?;
    db.fail_next_flush_for_test();
    let error = svc
        .workspace_put_impl_at(params("failed"), "fu2", now)
        .expect_err("flush failure cannot produce a successful ACK");
    assert_eq!(code(&error), "STORAGE_WRITE_FAILED");
    assert!(
        error
            .message
            .contains("injected explicit WAL flush failure")
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, b"fu2-neighbor")?,
        Some(b"unchanged".to_vec())
    );
    // Separate read records the permitted unacknowledged write, without claiming durability.
    let row = read_workspace_row_optional(&db, &key, now)?.expect("write precedes flush");
    assert_eq!(row.entry.value, Some(json!({"exact":56})));
    assert_eq!(row.entry.version, 1);
    Ok(())
}

#[test]
fn must_pass_put_after_one_shot_flush_fault_acknowledges_exact_readback() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.workspace_db()?;
    db.fail_next_flush_for_test();
    assert_eq!(
        db.flush().expect_err("precondition fault consumed").code(),
        "STORAGE_WRITE_FAILED"
    );
    let now = now_ns() / 1_000_000;
    let key = workspace_row_key("fu2-flush", "healthy");
    assert_eq!(db.get_cf_exact(cf::CF_KV, key.as_bytes())?, None);
    let ack = svc.workspace_put_impl_at(params("healthy"), "fu2", now)?;
    assert_eq!(ack.version, 1);
    let row = read_workspace_row_optional(&db, &key, now)?.expect("separate physical read");
    assert_eq!(row.entry.value, Some(json!({"exact":56})));
    assert_eq!(row.entry.version, 1);
    Ok(())
}
