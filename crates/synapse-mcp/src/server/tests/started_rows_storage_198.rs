//! #198. Parent: m4_tools. Real audited spawn path; no process launched.
use super::*;
use crate::server::g7a_support as f;

#[tokio::test(flavor = "current_thread")]
async fn must_fail_registry_read_after_started_still_writes_terminal_action_error() {
    let (_dir, mut svc) = f::service();
    // D8 (#2419): this row is about an allowed spawn, so the owner's switch is on.
    svc.m4_config = std::mem::take(&mut svc.m4_config).with_agent_spawn(
        crate::server::agent_spawn_switch::AgentSpawnSwitch::from_env_value(Some("1")),
    );
    let registry = Arc::clone(svc.session_registry_ref());
    let panicked = std::thread::spawn(move || {
        let _held = registry.lock().expect("registry");
        panic!("synthetic pre-spawn registry read failure");
    })
    .join();
    assert!(panicked.is_err());
    assert!(svc.session_registry_ref().is_poisoned());
    assert_eq!(f::audit_rows(&svc).len(), 0);
    let events = f::db(&svc)
        .scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)
        .expect("events");
    // #308: the facade resolves the omitted mcp_url against this daemon's
    // bind; the test bind is never the live daemon's port.
    let params: ActSpawnAgentParams = serde_json::from_value::<crate::m4::ActSpawnAgentRequest>(
        json!({"cli":"claude", "prompt":"never launch: registry fault"}),
    )
    .expect("valid request")
    .into_direct_spawn_params("127.0.0.1:47198")
    .expect("valid params");
    let trail = SpawnAuditTrail::new(SpawnAuditSink::Dashboard, &params, None);
    let error = svc
        .spawn_agent_audited(trail, params)
        .await
        .expect_err("registry read fails");
    assert_eq!(f::code(&error), error_codes::TOOL_INTERNAL_ERROR);
    let actions: Vec<_> = f::audit_rows(&svc)
        .into_iter()
        .filter(|r| r["tool"] == "act_spawn_agent" && r.get("status").is_some())
        .collect();
    assert_eq!(
        actions.len(),
        2,
        "started and terminal rows must both exist: {actions:?}"
    );
    assert_eq!(actions[0]["status"], "started");
    assert_eq!(actions[1]["status"], "error");
    assert_eq!(actions[1]["error_code"], error_codes::TOOL_INTERNAL_ERROR);
    assert_eq!(
        f::db(&svc)
            .scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)
            .expect("events"),
        events,
        "prepare failed before launch/journal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_valid_lease_status_facade_keeps_successful_final_audit() {
    let _lock = crate::test_support::lease_serial("g7a198");
    let (_dir, svc) = f::service();
    f::seed(&svc, "g7a198", f::now());
    assert_eq!(f::audit_rows(&svc).len(), 0);
    let params = serde_json::from_value(json!({"operation":"lease_status"})).expect("act params");
    let context = crate::test_support::request_context_for_session("g7a198").await;
    let response = svc.act(Parameters(params), context).await.expect("status");
    assert_eq!(
        serde_json::to_value(response.0).expect("response")["operation"],
        "lease_status"
    );
    let finals: Vec<_> = f::audit_rows(&svc)
        .into_iter()
        .filter(|r| r["tool"] == "act" && r["phase"] == "final")
        .collect();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0]["outcome"], "ok");
    assert_eq!(finals[0]["error_code"], Value::Null);
    assert!(!synapse_action::lease::status().held);
}
