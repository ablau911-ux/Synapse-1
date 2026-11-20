//! Frozen criteria_b2.md:4,13,15. Public handlers, isolated storage.
use crate::server::SynapseService;
use crate::test_support::{request_context_for_session, service_in};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;
use synapse_core::error_codes;
use synapse_storage::cf;

fn code(error: &rmcp::ErrorData) -> Option<&str> {
    error.data.as_ref()?.get("code")?.as_str()
}

#[tokio::test]
async fn must_fail_find_zero_limit_never_returns_a_match_277() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let service = service_in(dir.path())?;
    service.m1_state()?.synthetic = Some(crate::m1::synthetic_notepad_input());
    let positive = service
        .find(
            Parameters(serde_json::from_value(json!({"limit":1}))?),
            request_context_for_session("author-b2-find").await,
        )
        .await?;
    assert_eq!(positive.0.results.len(), 1, "fixture must have a match");
    let result = service
        .find(
            Parameters(serde_json::from_value(json!({"limit":0}))?),
            request_context_for_session("author-b2-find").await,
        )
        .await;
    match result {
        Ok(response) => assert!(
            response.0.results.is_empty(),
            "limit=0 returned {:?}",
            response.0
        ),
        Err(error) => assert_eq!(
            code(&error),
            Some(error_codes::TOOL_PARAMS_INVALID),
            "{error:?}"
        ),
    }
    Ok(())
}

#[tokio::test]
async fn must_pass_explicit_empty_include_omits_optional_diagnostics_279() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let service = service_in(dir.path())?;
    service.m1_state()?.synthetic = Some(crate::m1::synthetic_notepad_input());
    let omitted = service
        .observe(
            Parameters(serde_json::from_value(json!({}))?),
            request_context_for_session("author-b2-observe").await,
        )
        .await?;
    assert!(
        omitted.0.diagnostics.capture_config.is_some(),
        "omitted include keeps defaults"
    );
    let explicit = service
        .observe(
            Parameters(serde_json::from_value(json!({"include":[]}))?),
            request_context_for_session("author-b2-observe").await,
        )
        .await?;
    let wire = serde_json::to_value(explicit.0)?;
    for field in [
        "capture_config",
        "capture_runtime",
        "input_backends",
        "cdp",
        "web_path",
        "elements_page",
    ] {
        assert!(
            wire["diagnostics"].get(field).is_none(),
            "include:[] emitted {field}: {wire}"
        );
    }
    Ok(())
}

fn audit_db(service: &SynapseService) -> anyhow::Result<std::sync::Arc<synapse_storage::Db>> {
    let runtime = service.reflex_runtime()?;
    let guard = runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("runtime lock"))?;
    Ok(guard.storage_db())
}

#[tokio::test]
async fn must_fail_history_handler_preserves_flush_storage_code_275() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let service = service_in(dir.path())?;
    let db = audit_db(&service)?;
    let before = db.scan_cf(cf::CF_REFLEX_AUDIT)?;
    for invalid in [json!({"limit":1001}), json!({"reflex_id":" ","limit":1})] {
        let result = service
            .reflex_history(Parameters(serde_json::from_value(invalid)?))
            .await;
        let error = result
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid parameters accepted"))?;
        assert_eq!(code(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    }
    db.fail_next_flush_for_test();
    let result = service
        .reflex_history(Parameters(serde_json::from_value(json!({"limit":1}))?))
        .await;
    assert_eq!(db.scan_cf(cf::CF_REFLEX_AUDIT)?, before);
    let error = result
        .err()
        .ok_or_else(|| anyhow::anyhow!("flush failure acknowledged"))?;
    assert!(
        error
            .message
            .contains("injected explicit WAL flush failure"),
        "wrong failure: {error:?}"
    );
    assert_eq!(
        code(&error),
        Some(error_codes::STORAGE_WRITE_FAILED),
        "{error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_health_handler_preserves_audit_scan_storage_code_275() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let service = service_in(dir.path())?;
    let db = audit_db(&service)?;
    let before = db.scan_cf(cf::CF_REFLEX_AUDIT)?;
    // Assert the typed error itself. ParamsInvalid's Display can contain a
    // nested STORAGE_READ_FAILED and would satisfy the old text-only check.
    db.fail_next_read_for_test(cf::CF_REFLEX_AUDIT, b"");
    let runtime = service.reflex_runtime()?;
    let error = runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("runtime lock"))?
        .recursion_clamps_total()
        .expect_err("health's real counter must report the read fault");
    assert_eq!(error.code(), error_codes::STORAGE_READ_FAILED, "{error:?}");
    db.fail_next_read_for_test(cf::CF_REFLEX_AUDIT, b"");
    let health = service
        .health(request_context_for_session("author-b2-health").await)
        .await?;
    assert_eq!(db.scan_cf(cf::CF_REFLEX_AUDIT)?, before);
    let reflex = &health.0.subsystems["reflex"];
    assert_eq!(
        reflex.status, "error",
        "scan fault must reach real health path"
    );
    let detail = reflex.detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("injected storage read IO failure"),
        "wrong failure: {detail}"
    );
    assert!(
        detail.contains(error_codes::STORAGE_READ_FAILED),
        "storage code lost: {detail}"
    );
    assert!(
        !detail.contains(error_codes::REFLEX_PARAMS_INVALID),
        "{detail}"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_history_scan_preserves_storage_error_code_275() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let service = service_in(dir.path())?;
    let db = audit_db(&service)?;
    let runtime = service.reflex_runtime()?;
    for reflex_id in [None, Some("missing-275")] {
        db.fail_next_read_for_test(cf::CF_REFLEX_AUDIT, b"");
        let error = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime lock"))?
            .history(reflex_id, 1)
            .expect_err("audit scan IO failure");
        assert_eq!(
            error.code(),
            error_codes::STORAGE_READ_FAILED,
            "history({reflex_id:?}): {error:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn must_fail_status_scans_preserve_storage_error_code_275() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let service = service_in(dir.path())?;
    let db = audit_db(&service)?;
    let runtime = service.reflex_runtime()?;
    db.fail_next_read_for_test(cf::CF_REFLEX_AUDIT, b"");
    let error = runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("runtime lock"))?
        .list(true)
        .expect_err("terminal status scan IO failure");
    assert_eq!(error.code(), error_codes::STORAGE_READ_FAILED, "{error:?}");
    db.fail_next_read_for_test(cf::CF_REFLEX_AUDIT, b"");
    let error = runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("runtime lock"))?
        .cancel("missing-275")
        .expect_err("single terminal status scan IO failure");
    assert_eq!(error.code(), error_codes::STORAGE_READ_FAILED, "{error:?}");
    Ok(())
}

#[tokio::test]
async fn must_fail_history_and_status_decode_preserve_storage_error_code_275() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let service = service_in(dir.path())?;
    let db = audit_db(&service)?;
    let key = b"corrupt-275:1".to_vec();
    db.put_batch_pressure_bypass(cf::CF_REFLEX_AUDIT, [(key.clone(), b"not-json".to_vec())])?;
    let runtime = service.reflex_runtime()?;
    let mut runtime = runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("runtime lock"))?;
    for error in [
        runtime.history(None, 1).expect_err("history decode"),
        runtime
            .history(Some("corrupt-275"), 1)
            .expect_err("prefix history decode"),
        runtime.list(true).expect_err("terminal status decode"),
        runtime
            .cancel("corrupt-275")
            .expect_err("single terminal status decode"),
    ] {
        // DecodeJson currently shares STORAGE_READ_FAILED with read faults.
        assert_eq!(error.code(), error_codes::STORAGE_READ_FAILED, "{error:?}");
    }
    assert_eq!(
        db.get_cf_exact(cf::CF_REFLEX_AUDIT, &key)?,
        Some(b"not-json".to_vec())
    );
    Ok(())
}
