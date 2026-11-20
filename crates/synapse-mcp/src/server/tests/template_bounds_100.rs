use super::*;
use crate::server::g7b_support::{code, service};

fn seed(svc: &SynapseService) -> anyhow::Result<()> {
    for id in ["g7b-a", "g7b-b"] {
        svc.agent_template_put_impl(AgentTemplatePutParams {
            template_id: id.into(),
            name: id.into(),
            description: "boundary fixture".into(),
            model: "codex".into(),
            directory: None,
            prompt: "do nothing".into(),
        })?;
    }
    Ok(())
}

#[tokio::test]
async fn must_pass_template_list_honors_both_schema_boundaries_100() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    for max in [1, 1000] {
        let empty = svc
            .agent_template_list(
                Parameters(AgentTemplateListParams { max }),
                crate::test_support::request_context_for_session("template-bounds-100").await,
            )
            .await?
            .0;
        assert_eq!(empty.count, 0);
    }
    seed(&svc)?;
    let db = svc.agent_template_db()?;
    let before = db.scan_cf_prefix(cf::CF_KV, template_prefix().as_bytes())?;
    assert_eq!(before.len(), 2);
    for (max, count) in [(1, 1), (1000, 2)] {
        let result = svc
            .agent_template_list(
                Parameters(AgentTemplateListParams { max }),
                crate::test_support::request_context_for_session("template-bounds-100").await,
            )
            .await?
            .0;
        assert_eq!(result.count, count);
        assert_eq!(result.templates[0].template_id, "g7b-a");
        assert_eq!(result.templates.len(), count);
    }
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, template_prefix().as_bytes())?,
        before
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_template_list_invalid_bounds_leave_empty_and_nonempty_store_100()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.agent_template_db()?;
    for populated in [false, true] {
        if populated {
            seed(&svc)?;
        }
        let before = db.scan_cf(cf::CF_KV)?;
        assert_eq!(
            db.scan_cf_prefix(cf::CF_KV, template_prefix().as_bytes())?
                .len(),
            if populated { 2 } else { 0 }
        );
        for max in [0, 1001, usize::MAX] {
            let error = svc
                .agent_template_list(
                    Parameters(AgentTemplateListParams { max }),
                    crate::test_support::request_context_for_session("template-bounds-100").await,
                )
                .await
                .err()
                .expect("outside schema bounds");
            assert_eq!(code(&error), "TOOL_PARAMS_INVALID");
            assert_eq!(db.scan_cf(cf::CF_KV)?, before);
        }
    }
    Ok(())
}
