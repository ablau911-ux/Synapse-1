//! #299 MY TDD contract: several agents inside ONE MCP session, each with its
//! own target. Every lookup and bind goes through the same service calls the
//! tools use (`session_target`, `set_session_target`, `clear_session_target`,
//! `clear_session_cdp_target_if_matches`), run inside `AgentScope::scope`
//! exactly as `call_tool` runs a call that carries `agent_id`.

use serde_json::{Map, Value, json};
use synapse_core::error_codes;
use tempfile::TempDir;

use super::{ErrorData, SessionTarget, SynapseService, tests::service_with_temp_db};
use crate::server::{
    agent_scope::{self, AGENT_ID_FIELD, AGENT_SCOPED_TOOLS, AgentScope},
    schema_sanitize::sanitize_tools,
    tool_profiles::PUBLIC_TOOL_NAMES,
};

fn tab(hwnd: i64, id: &str) -> SessionTarget {
    SessionTarget::Cdp {
        window_hwnd: hwnd,
        cdp_target_id: id.to_owned(),
    }
}

/// Runs `call` as agent `agent` of `session`, the way `call_tool` runs a call
/// whose arguments carried `agent_id`.
async fn as_agent<T>(session: &str, agent: &str, call: impl FnOnce() -> T) -> T {
    AgentScope::scope(Some(AgentScope::new(session, agent)), async move { call() }).await
}

fn error_code(error: &ErrorData) -> Option<String> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn arguments(value: Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}

#[tokio::test]
async fn must_fail_two_agents_of_one_session_keep_their_own_targets() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-two-agents";
    service.set_session_target(session, tab(0x2990, "chrome-tab:2990"))?;
    as_agent(session, "walk-a", || {
        service.set_session_target(session, tab(0x2991, "chrome-tab:2991"))
    })
    .await?;
    as_agent(session, "walk-b", || {
        service.set_session_target(session, tab(0x2992, "chrome-tab:2992"))
    })
    .await?;

    let seen_by_a = as_agent(session, "walk-a", || service.session_target(Some(session))).await?;
    let seen_by_b = as_agent(session, "walk-b", || service.session_target(Some(session))).await?;
    assert_eq!(seen_by_a, Some(tab(0x2991, "chrome-tab:2991")));
    assert_eq!(seen_by_b, Some(tab(0x2992, "chrome-tab:2992")));
    // Neither agent's bind replaced the session's own target.
    assert_eq!(
        service.memory_session_target(session)?,
        Some(tab(0x2990, "chrome-tab:2990"))
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_an_agent_without_a_binding_never_gets_another_target() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-unbound-agent";
    service.set_session_target(session, tab(0x2993, "chrome-tab:2993"))?;
    as_agent(session, "walk-a", || {
        service.set_session_target(session, tab(0x2994, "chrome-tab:2994"))
    })
    .await?;

    let seen_by_c = as_agent(session, "walk-c", || service.session_target(Some(session))).await?;
    assert_eq!(seen_by_c, None, "walk-c bound nothing, so it has no target");
    Ok(())
}

#[tokio::test]
async fn must_fail_an_unscoped_lookup_sees_no_target_while_agents_hold_targets()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-unscoped";
    let own = tab(0x2995, "chrome-tab:2995");
    service.set_session_target(session, own.clone())?;
    as_agent(session, "walk-a", || {
        service.set_session_target(session, tab(0x2996, "chrome-tab:2996"))
    })
    .await?;

    assert_eq!(
        service.session_target(Some(session))?,
        None,
        "a call that names no agent must not reach any tab while agents hold targets"
    );

    let cleared = as_agent(session, "walk-a", || service.clear_session_target(session)).await?;
    assert!(
        cleared.is_some(),
        "walk-a clear returns walk-a's own target"
    );
    assert_eq!(
        service.session_target(Some(session))?,
        Some(own),
        "with no agent target left, the session's own target is back"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_two_agents_cannot_bind_the_same_tab() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-same-tab";
    as_agent(session, "walk-a", || {
        service.set_session_target(session, tab(0x2997, "chrome-tab:2997"))
    })
    .await?;

    let refusal = as_agent(session, "walk-b", || {
        service.set_session_target(session, tab(0x2997, "CHROME-TAB:2997"))
    })
    .await
    .expect_err("walk-b must not bind the tab walk-a drives");
    assert_eq!(
        error_code(&refusal).as_deref(),
        Some(error_codes::ACTION_TARGET_INVALID)
    );
    assert!(refusal.message.contains("walk-a"), "{}", refusal.message);
    let seen_by_b = as_agent(session, "walk-b", || service.session_target(Some(session))).await?;
    assert_eq!(seen_by_b, None);
    Ok(())
}

#[tokio::test]
async fn must_fail_closing_a_tab_clears_it_from_every_agent_that_holds_it() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-closed-tab";
    as_agent(session, "walk-a", || {
        service.set_session_target(session, tab(0x2998, "chrome-tab:2998"))
    })
    .await?;
    as_agent(session, "walk-b", || {
        service.set_session_target(session, tab(0x2999, "chrome-tab:2999"))
    })
    .await?;

    let previous = as_agent(session, "walk-a", || {
        service.clear_session_cdp_target_if_matches(session, "chrome-tab:2998")
    })
    .await?;
    assert!(
        previous.is_some(),
        "walk-a's closed tab is reported as cleared"
    );

    let seen_by_a = as_agent(session, "walk-a", || service.session_target(Some(session))).await?;
    let seen_by_b = as_agent(session, "walk-b", || service.session_target(Some(session))).await?;
    assert_eq!(seen_by_a, None);
    assert_eq!(seen_by_b, Some(tab(0x2999, "chrome-tab:2999")));
    Ok(())
}

#[tokio::test]
async fn must_fail_session_teardown_forgets_agent_targets() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-teardown";
    as_agent(session, "walk-a", || {
        service.set_session_target(session, tab(0x299a, "chrome-tab:299a"))
    })
    .await?;

    assert_eq!(agent_scope::forget_session(session), 1);
    let seen_by_a = as_agent(session, "walk-a", || service.session_target(Some(session))).await?;
    assert_eq!(seen_by_a, None);
    Ok(())
}

#[tokio::test]
async fn must_fail_a_target_tool_call_without_agent_id_is_refused_while_agents_hold_targets()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i299-admission";
    let mut plain = arguments(json!({"operation": "navigate", "url": "about:blank"}));
    assert_eq!(
        agent_scope::scope_for_call("browser_nav", Some(session), &mut plain)?,
        None,
        "no agent holds a target yet: the call runs exactly as before #299"
    );

    as_agent(session, "walk-a", || {
        service.set_session_target(session, tab(0x299b, "chrome-tab:299b"))
    })
    .await?;

    let mut forgot = arguments(json!({"operation": "invoke", "action": {"verb": "read"}}));
    let refusal = agent_scope::scope_for_call("act", Some(session), &mut forgot)
        .expect_err("act without agent_id must be refused while walk-a holds a target");
    assert_eq!(
        error_code(&refusal).as_deref(),
        Some(error_codes::TOOL_PARAMS_INVALID)
    );
    assert!(
        refusal.message.contains("walk-a") && refusal.message.contains(AGENT_ID_FIELD),
        "{}",
        refusal.message
    );

    let mut health = arguments(json!({}));
    assert_eq!(
        agent_scope::scope_for_call("health", Some(session), &mut health)?,
        None,
        "tools without a target are not affected"
    );

    let mut scoped =
        arguments(json!({"operation": "invoke", "action": {"verb": "read"}, "agent_id": "walk-b"}));
    let scope = agent_scope::scope_for_call("act", Some(session), &mut scoped)?;
    assert_eq!(scope, Some(AgentScope::new(session, "walk-b")));
    assert!(
        scoped
            .as_ref()
            .is_some_and(|arguments| !arguments.contains_key(AGENT_ID_FIELD)),
        "agent_id is taken out before the tool's own parameters are parsed"
    );
    Ok(())
}

#[test]
fn must_fail_a_malformed_agent_id_is_refused() {
    for bad in [
        json!(""),
        json!("walk a"),
        json!("walk/a"),
        json!("x".repeat(65)),
        json!(7),
        Value::Null,
        json!(false),
        json!(["walk-a"]),
        json!({"id": "walk-a"}),
    ] {
        let mut call = arguments(json!({"operation": "get", AGENT_ID_FIELD: bad.clone()}));
        let refusal = agent_scope::scope_for_call("target", Some("i299-bad-id"), &mut call)
            .expect_err("malformed agent_id must be refused");
        assert_eq!(
            error_code(&refusal).as_deref(),
            Some(error_codes::TOOL_PARAMS_INVALID),
            "{bad}"
        );
    }
    let mut sessionless = arguments(json!({"operation": "get", AGENT_ID_FIELD: "walk-a"}));
    assert!(
        agent_scope::scope_for_call("target", None, &mut sessionless).is_err(),
        "agent_id needs an MCP session"
    );
}

#[test]
fn must_fail_every_agent_scoped_tool_lists_agent_id_in_its_schema() {
    let tools = sanitize_tools(SynapseService::tool_router().list_all());
    for name in AGENT_SCOPED_TOOLS {
        assert!(
            PUBLIC_TOOL_NAMES.contains(name),
            "{name} is not a public tool name"
        );
        let tool = tools
            .iter()
            .find(|tool| tool.name.as_ref() == *name)
            .unwrap_or_else(|| panic!("{name} missing from the built tool surface"));
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(
            schema["properties"][AGENT_ID_FIELD]["type"], "string",
            "{name}: {schema}"
        );
        assert_eq!(
            schema["properties"][AGENT_ID_FIELD]["pattern"],
            "^[A-Za-z0-9._:-]+$"
        );
        for keyword in ["oneOf", "anyOf"] {
            for branch in schema[keyword].as_array().into_iter().flatten() {
                if branch["additionalProperties"] == Value::Bool(false) {
                    assert_eq!(
                        branch["properties"][AGENT_ID_FIELD]["$ref"], "#/properties/agent_id",
                        "{name}: a closed {keyword} branch must accept agent_id: {branch}"
                    );
                }
            }
        }
    }
    for tool in &tools {
        if !AGENT_SCOPED_TOOLS.contains(&tool.name.as_ref()) {
            assert!(
                tool.input_schema
                    .get("properties")
                    .and_then(|properties| properties.get(AGENT_ID_FIELD))
                    .is_none(),
                "{} must not advertise agent_id",
                tool.name
            );
        }
    }
}

#[test]
fn must_fail_the_live_tools_list_carries_agent_id() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let tools = service.full_sanitized_tools();
    let act = tools
        .iter()
        .find(|tool| tool.name.as_ref() == "act")
        .expect("act tool");
    assert!(
        act.input_schema
            .get("properties")
            .and_then(|properties| properties.get(AGENT_ID_FIELD))
            .is_some(),
        "tools/list must offer agent_id on act"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_agent_299_window_alias_cannot_bypass_a_tab_binding() -> anyhow::Result<()> {
    let session = "i299-window-alias";
    let held = tab(0x29c0, "chrome-tab:29c0");
    agent_scope::bind(session, "walk-a", held.clone())?;
    let window = SessionTarget::Window { hwnd: 0x29c0 };

    assert!(agent_scope::ensure_access(session, &held).is_err());
    assert!(agent_scope::bind(session, "walk-b", window.clone()).is_err());
    let refusal = as_agent(session, "walk-b", || {
        agent_scope::ensure_access(session, &window)
    })
    .await
    .expect_err("a window read would include the other agent's tab");
    assert_eq!(
        error_code(&refusal).as_deref(),
        Some(error_codes::ACTION_TARGET_INVALID)
    );
    as_agent(session, "walk-a", || {
        agent_scope::ensure_access(session, &held)
    })
    .await?;
    agent_scope::bind(session, "walk-b", tab(0x29c0, "chrome-tab:29c1"))?;
    assert_eq!(agent_scope::forget_session(session), 2);

    agent_scope::bind(session, "walk-a", window)?;
    assert!(agent_scope::bind(session, "walk-b", held).is_err());
    assert_eq!(agent_scope::forget_session(session), 1);
    Ok(())
}

#[test]
fn must_fail_agent_299_unsupported_tools_reject_agent_id() {
    for id in [json!("walk-a"), Value::Null, json!(7)] {
        let mut call = arguments(json!({AGENT_ID_FIELD: id}));
        let refusal = agent_scope::scope_for_call("health", Some("i299-unsupported"), &mut call)
            .expect_err("an unsupported agent_id must not be silently ignored");
        assert_eq!(
            error_code(&refusal).as_deref(),
            Some(error_codes::TOOL_PARAMS_INVALID)
        );
    }
}

#[test]
fn must_fail_agent_299_legacy_live_target_operations_are_refused() -> anyhow::Result<()> {
    let session = "i299-legacy-tools";
    agent_scope::bind(session, "walk-a", tab(0x29c2, "chrome-tab:29c2"))?;
    for (tool, operation) in [
        ("livecapture", "next"),
        ("livefeed", "look"),
        ("subscribe", "events"),
        ("replay", "record"),
        ("replay", "demo_start"),
        ("verification", "inbox"),
        ("verification", "poll"),
        ("routine", "armed_tick"),
        ("assist", "suggestion_accept"),
    ] {
        let mut call = arguments(json!({"operation": operation}));
        let refusal = agent_scope::scope_for_call(tool, Some(session), &mut call)
            .expect_err("legacy live-target tools cannot bypass an agent binding");
        assert!(refusal.message.contains("walk-a"), "{}", refusal.message);
    }
    let mut call = arguments(json!({"operation": "list"}));
    assert_eq!(
        agent_scope::scope_for_call("workspace", Some(session), &mut call)?,
        None
    );
    agent_scope::forget_session(session);
    let mut call = arguments(json!({"operation": "look"}));
    assert_eq!(
        agent_scope::scope_for_call("livefeed", Some(session), &mut call)?,
        None
    );
    Ok(())
}
