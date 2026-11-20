//! #97: fail a real per-DB storage read, not a JSON decoder or error mapper.
use super::*;
use crate::server::g7b_support::{code, service};
use synapse_storage::cf;

const ID: &str = "sg1-00000000000000000000000000000097";

#[tokio::test]
async fn must_fail_server_storage_read_io_is_not_mapped_to_absence() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    assert_eq!(load_suggestion_by_id(&db, ID)?, None);
    let before = db.scan_cf(cf::CF_KV)?;
    for dry_run in [true, false] {
        db.fail_next_read_for_test(cf::CF_KV, b"");
        let error = svc
            .suggestion_accept_impl(
                SuggestionAcceptParams {
                    suggestion_id: ID.into(),
                    dry_run,
                    browser_window_hwnd: None,
                    launch_timeout_ms: Some(1000),
                },
                Some("fu2-97".into()),
            )
            .await
            .expect_err("physical read fault");
        assert_eq!(code(&error), "STORAGE_READ_FAILED");
        assert!(error.message.contains("injected storage read IO failure"));
        assert_eq!(db.scan_cf(cf::CF_KV)?, before);
        assert_eq!(load_suggestion_by_id(&db, ID)?, None);
    }
    Ok(())
}

#[tokio::test]
async fn must_pass_read_fault_on_other_database_does_not_change_domain_absence()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let other_dir = tempfile::tempdir()?;
    let other = synapse_storage::Db::open(other_dir.path(), synapse_core::SCHEMA_VERSION)?;
    assert_eq!(load_suggestion_by_id(&db, ID)?, None);
    other.fail_next_read_for_test(cf::CF_KV, b"");
    let error = svc
        .suggestion_accept_impl(
            SuggestionAcceptParams {
                suggestion_id: ID.into(),
                dry_run: true,
                browser_window_hwnd: None,
                launch_timeout_ms: Some(1000),
            },
            Some("fu2-97".into()),
        )
        .await
        .expect_err("absent domain record");
    assert_eq!(code(&error), "SUGGESTION_NOT_FOUND");
    assert_eq!(load_suggestion_by_id(&db, ID)?, None);
    assert_eq!(
        other
            .get_cf_exact(cf::CF_KV, b"probe")
            .expect_err("other fault still armed")
            .code(),
        "STORAGE_READ_FAILED"
    );
    Ok(())
}
