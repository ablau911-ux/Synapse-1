//! #299 MY TDD contract: `browser_form` honours an explicit per-call tab the
//! calling MCP session OWNS even when it is not the session's active target,
//! the same ownership rule `browser_nav` already uses, and it still refuses a
//! tab this session does not own.

use synapse_core::error_codes;
use tempfile::TempDir;

use super::{
    CdpTargetOwner, ErrorData, SessionTarget, SynapseService,
    tests::{seed_session_client, service_with_temp_db},
};
use crate::server::browser_field::BrowserSetValueParams;

const CLIENT: &str = "claude-code";

fn register_owner(
    service: &SynapseService,
    session: &str,
    hwnd: i64,
    tab: &str,
) -> anyhow::Result<()> {
    let now = crate::server::session_registry::unix_time_ms_now();
    seed_session_client(service, session, CLIENT, now)?;
    service.register_cdp_target_owner(CdpTargetOwner {
        session_id: session.to_owned(),
        window_hwnd: hwnd,
        endpoint: "chrome-extension://test/chrome.tabs".to_owned(),
        chrome_window_id: None,
        capture_window_hwnd: None,
        cdp_target_id: tab.to_owned(),
        requested_url: "about:blank#299".to_owned(),
        target_url: "about:blank#299".to_owned(),
        created_at_unix_ms: now,
    })?;
    Ok(())
}

fn form_call(hwnd: i64, tab: &str) -> BrowserSetValueParams {
    BrowserSetValueParams {
        text: String::new(),
        selector: Some("#q".to_owned()),
        element_id: None,
        active_element: false,
        cdp_target_id: Some(tab.to_owned()),
        window_hwnd: Some(hwnd),
    }
}

fn error_code(error: &ErrorData) -> Option<String> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

#[test]
fn must_fail_browser_form_drives_an_owned_tab_that_is_not_the_active_target() -> anyhow::Result<()>
{
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-form-owned";
    register_owner(&service, session, 0x29a1, "chrome-tab:29a1")?;
    service.set_session_target(
        session,
        SessionTarget::Cdp {
            window_hwnd: 0x29a2,
            cdp_target_id: "chrome-tab:29a2".to_owned(),
        },
    )?;

    let resolved = service.resolve_bridge_target(session, &form_call(0x29a1, "chrome-tab:29a1"))?;
    assert_eq!(resolved, (0x29a1, "chrome-tab:29a1".to_owned()));
    Ok(())
}

#[test]
fn must_fail_browser_form_refuses_a_tab_this_session_does_not_own() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-form-unowned";
    let now = crate::server::session_registry::unix_time_ms_now();
    seed_session_client(&service, session, CLIENT, now)?;
    service.set_session_target(
        session,
        SessionTarget::Cdp {
            window_hwnd: 0x29a2,
            cdp_target_id: "chrome-tab:29a2".to_owned(),
        },
    )?;
    register_owner(
        &service,
        "i299-form-other-session",
        0x29a4,
        "chrome-tab:29a4",
    )?;

    for (hwnd, tab) in [(0x29a3, "chrome-tab:29a3"), (0x29a4, "chrome-tab:29a4")] {
        let refusal = service
            .resolve_bridge_target(session, &form_call(hwnd, tab))
            .expect_err("a tab this session does not own must be refused");
        assert_eq!(
            error_code(&refusal).as_deref(),
            Some(error_codes::ACTION_TARGET_INVALID),
            "{tab}: {}",
            refusal.message
        );
    }
    Ok(())
}

#[tokio::test]
async fn must_fail_agent_299_explicit_owned_tab_cannot_cross_an_agent_binding() -> anyhow::Result<()>
{
    use crate::server::agent_scope::AgentScope;

    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-form-agents";
    let hwnd = 0x29d0;
    let tab_a = "chrome-tab:29d0";
    let tab_b = "chrome-tab:29d1";
    register_owner(&service, session, hwnd, tab_a)?;
    register_owner(&service, session, hwnd, tab_b)?;
    AgentScope::scope(Some(AgentScope::new(session, "walk-a")), async {
        service.set_session_target(
            session,
            SessionTarget::Cdp {
                window_hwnd: hwnd,
                cdp_target_id: tab_a.to_owned(),
            },
        )
    })
    .await?;

    AgentScope::scope(Some(AgentScope::new(session, "walk-b")), async {
        let refusal = service
            .resolve_bridge_target(session, &form_call(hwnd, tab_a))
            .expect_err("session ownership must not override walk-a's binding");
        assert_eq!(
            error_code(&refusal).as_deref(),
            Some(error_codes::ACTION_TARGET_INVALID)
        );
        assert!(
            service
                .cdp_target_owner_for_readback("test", session, tab_a)
                .is_err()
        );
        assert!(
            service
                .cdp_target_owner_for_close(session, tab_a, None)
                .is_err()
        );
        assert_eq!(
            service.resolve_bridge_target(session, &form_call(hwnd, tab_b))?,
            (hwnd, tab_b.to_owned()),
            "another unbound owned tab in the same browser remains usable"
        );
        Ok::<_, ErrorData>(())
    })
    .await?;

    // Screenshot setup uses the same synchronous scope inside spawn_blocking.
    let blocking_service = service.clone();
    let scope = AgentScope::new(session, "walk-a");
    let resolved = tokio::task::spawn_blocking(move || {
        AgentScope::sync_scope(Some(scope), || {
            blocking_service.resolve_bridge_target(session, &form_call(hwnd, tab_a))
        })
    })
    .await??;
    assert_eq!(resolved, (hwnd, tab_a.to_owned()));
    assert!(
        service
            .resolve_bridge_target(session, &form_call(hwnd, tab_a))
            .is_err()
    );
    crate::server::agent_scope::forget_session(session);
    Ok(())
}
