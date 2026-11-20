//! Regression tests for #56: the daemon-wide reaper removes expired workspace
//! rows in every run namespace, not only the namespace being written.

use serde_json::json;
use tempfile::TempDir;

use super::super::workspace_blackboard_tests::service_with_db;
use super::super::{WorkspacePutParams, workspace_row_key};
use super::*;

const PUT_AT_MS: u64 = 1_000_000;

fn put(
    service: &crate::server::SynapseService,
    run_id: &str,
    key: &str,
    ttl_ms: u64,
) -> anyhow::Result<()> {
    service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
            expected_version: None,
            value: Some(json!({ "run": run_id })),
            artifact: None,
            ttl_ms,
        },
        "reaper-session",
        PUT_AT_MS,
    )?;
    Ok(())
}

fn row_exists(db: &Db, run_id: &str, key: &str) -> anyhow::Result<bool> {
    let row_key = workspace_row_key(run_id, key);
    Ok(db
        .scan_cf_prefix(cf::CF_KV, row_key.as_bytes())?
        .iter()
        .any(|(stored, _)| stored == row_key.as_bytes()))
}

#[test]
fn must_pass_sweep_reaps_expired_rows_across_every_run_namespace() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    put(&service, "dead-run-a", "notes/old", 10)?;
    put(&service, "dead-run-b", "notes/old", 10)?;
    put(&service, "live-run", "notes/fresh", 60_000)?;
    let db = service.workspace_db()?;

    let report = WorkspaceReaper::sweep(&db, PUT_AT_MS + 11)?;

    assert_eq!(report.expired_rows_deleted, 2);
    assert_eq!(report.scanned_rows, 3);
    assert!(!row_exists(&db, "dead-run-a", "notes/old")?);
    assert!(!row_exists(&db, "dead-run-b", "notes/old")?);
    assert!(row_exists(&db, "live-run", "notes/fresh")?);
    Ok(())
}

#[test]
fn must_fail_sweep_keeps_rows_until_their_ttl_elapses() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    put(&service, "run-boundary", "k", 10)?;
    let db = service.workspace_db()?;

    let before = WorkspaceReaper::sweep(&db, PUT_AT_MS + 9)?;
    assert_eq!(before.expired_rows_deleted, 0);
    assert!(row_exists(&db, "run-boundary", "k")?);

    let at_expiry = WorkspaceReaper::sweep(&db, PUT_AT_MS + 10)?;
    assert_eq!(at_expiry.expired_rows_deleted, 1);
    assert!(!row_exists(&db, "run-boundary", "k")?);
    Ok(())
}

#[test]
fn must_fail_sweep_skips_corrupt_rows_without_deleting_them() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    put(&service, "run-mixed", "good", 10)?;
    let db = service.workspace_db()?;
    let corrupt_key = workspace_row_key("run-mixed", "bad");
    db.put_batch_pressure_bypass(
        cf::CF_KV,
        [(corrupt_key.as_bytes().to_vec(), b"{not-json".to_vec())],
    )?;

    let report = WorkspaceReaper::sweep(&db, PUT_AT_MS + 11)?;

    assert_eq!(report.corrupt_rows_skipped, 1);
    assert_eq!(report.expired_rows_deleted, 1);
    assert!(row_exists(&db, "run-mixed", "bad")?);
    Ok(())
}

#[test]
fn must_fail_sweep_pages_through_more_rows_than_one_window() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    for index in 0..7 {
        put(&service, &format!("run-page-{index}"), "k", 10)?;
    }
    put(&service, "run-page-live", "k", 60_000)?;
    let db = service.workspace_db()?;

    let report = WorkspaceReaper::sweep_paged(&db, PUT_AT_MS + 11, 2)?;

    assert_eq!(report.scanned_rows, 8);
    assert_eq!(report.expired_rows_deleted, 7);
    // 8 rows at 2 per page with a one-row lookahead: exactly 4 pages (no
    // trailing empty page, no re-scan).
    assert_eq!(report.pages, 4);
    assert!(row_exists(&db, "run-page-live", "k")?);
    Ok(())
}

#[test]
fn must_fail_sweep_of_an_empty_store_is_a_single_empty_page() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let db = service.workspace_db()?;
    let report = WorkspaceReaper::sweep(&db, PUT_AT_MS)?;
    assert_eq!(
        report,
        WorkspaceReapReport {
            pages: 1,
            ..WorkspaceReapReport::default()
        }
    );
    Ok(())
}

#[test]
fn must_fail_sweep_deletes_an_expired_key_without_touching_its_prefix_siblings()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    put(&service, "run-sibling", "k", 10)?;
    // Row key hex of "k2" extends the row key hex of "k".
    service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some("run-sibling".to_owned()),
            key: "k2".to_owned(),
            expected_version: None,
            value: Some(json!({ "live": true })),
            artifact: None,
            ttl_ms: 60_000,
        },
        "reaper-session",
        PUT_AT_MS,
    )?;
    let db = service.workspace_db()?;

    let report = WorkspaceReaper::sweep(&db, PUT_AT_MS + 11)?;

    assert_eq!(report.expired_rows_deleted, 1);
    assert!(!row_exists(&db, "run-sibling", "k")?);
    assert!(row_exists(&db, "run-sibling", "k2")?);
    Ok(())
}
