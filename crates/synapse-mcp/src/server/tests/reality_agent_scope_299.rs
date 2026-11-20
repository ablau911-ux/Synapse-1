use super::{
    super::agent_scope::{self, AgentScope},
    tests::service_with_profile_dir,
};
use synapse_core::error_codes;
use tempfile::TempDir;

#[tokio::test]
async fn must_fail_agent_299_reality_cannot_sample_foreground_without_its_own_target()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_profile_dir(dir.path(), dir.path())?;
    let session = "i299-reality-unbound";
    let error = crate::http::with_current_mcp_session_id(
        Some(session.to_owned()),
        AgentScope::scope(Some(AgentScope::new(session, "walk-b")), async {
            service.reality_target_hwnd()
        }),
    )
    .await
    .expect_err("an unbound agent cannot use reality's foreground fallback");
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str),
        Some(error_codes::TARGET_NOT_SET)
    );

    agent_scope::bind(
        session,
        "walk-b",
        super::super::SessionTarget::Window { hwnd: 0x29e0 },
    )?;
    let own = crate::http::with_current_mcp_session_id(
        Some(session.to_owned()),
        AgentScope::scope(Some(AgentScope::new(session, "walk-b")), async {
            service.reality_target_hwnd()
        }),
    )
    .await?;
    assert_eq!(own, Some(0x29e0));
    agent_scope::forget_session(session);
    Ok(())
}
