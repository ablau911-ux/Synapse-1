//! Real server acceptance path: absence must not masquerade as an IO failure.
use super::*;
use crate::server::g7b_support::{code, service};
use synapse_storage::cf;

#[path = "claimed_execution_pressure_273.rs"]
mod claimed_execution_pressure_273;

#[tokio::test]
async fn must_fail_missing_suggestion_uses_domain_code_without_side_effect_97() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let id = "sg1-00000000000000000000000000000097";
    assert!(load_suggestion_by_id(&db, id)?.is_none());
    let before = db.scan_cf(cf::CF_KV)?;
    for dry_run in [true, false] {
        let params = SuggestionAcceptParams {
            suggestion_id: id.into(),
            dry_run,
            browser_window_hwnd: None,
            launch_timeout_ms: Some(1000),
        };
        let error = svc
            .suggestion_accept_impl(params, Some("g7b".into()))
            .await
            .expect_err("valid absent id");
        assert_eq!(code(&error), "SUGGESTION_NOT_FOUND");
        assert_eq!(db.scan_cf(cf::CF_KV)?, before);
        assert!(load_suggestion_by_id(&db, id)?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn must_fail_malformed_suggestion_stays_params_invalid_97() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let before = db.scan_cf(cf::CF_KV)?;
    let params = SuggestionAcceptParams {
        suggestion_id: String::new(),
        dry_run: true,
        browser_window_hwnd: None,
        launch_timeout_ms: Some(1000),
    };
    let error = svc
        .suggestion_accept_impl(params, None)
        .await
        .expect_err("malformed id");
    assert_eq!(code(&error), "TOOL_PARAMS_INVALID");
    assert_eq!(db.scan_cf(cf::CF_KV)?, before);
    Ok(())
}
