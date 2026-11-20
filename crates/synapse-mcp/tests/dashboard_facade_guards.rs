// SEAM NEEDED: the retired-name refusal below still drives a private copy of
// the guard. The real daemon always serves the public facades, so no daemon
// input can make a dashboard panel see only retired implementation names; it
// belongs in an in-crate unit test of src/http/dashboard_facade_guard.rs.
#[allow(dead_code)]
#[path = "../src/http/dashboard_facade_guard.rs"]
mod dashboard_facade_guard;

#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;

use dashboard_facade_guard::DashboardFacade;
use serde_json::{Value, json};
use support::HttpDaemon;

/// Dashboard panels gated by `DashboardFacade`, keyed as `/dashboard/state.json`
/// serves them, with the public facade that grants each one.
const GATED_PANELS: &[(&str, &str)] = &[
    ("tasks", "task"),
    ("approvals", "approval"),
    ("suggestions", "approval"),
    ("armed_runs", "approval"),
    ("hygiene", "hygiene"),
    ("local_models", "model"),
];

/// Real daemon: the facade names the daemon itself reports through the
/// `health` tool grant every gated dashboard panel, and the context panel's
/// workspace surface is available.
#[tokio::test]
async fn must_pass_dashboard_panels_accept_the_public_facades() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("dashboard-facade-guard-test", &[]).await?;
    let health = session.tools_call("health", json!({})).await?;
    let tool_names: BTreeSet<String> = health["structuredContent"]["tool_names"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for facade in ["workspace", "agent", "approval", "task", "model", "hygiene"] {
        assert!(
            tool_names.contains(facade),
            "daemon health must report public facade {facade}: {tool_names:?}"
        );
    }

    let state: Value = reqwest::Client::new()
        .get(daemon.url("/dashboard/state.json"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    for (panel, facade) in GATED_PANELS {
        assert_ne!(
            state[*panel]["status"], "unavailable",
            "panel {panel} must be granted by facade {facade}: {}",
            state[*panel]
        );
    }
    assert_eq!(
        state["context"]["data"]["workspace"]["available"],
        Value::Bool(true),
        "context workspace surface must be granted by the workspace facade: {}",
        state["context"]
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[test]
fn must_fail_retired_implementation_names_do_not_grant_dashboard_panels() {
    let retired = BTreeSet::from([
        "workspace_list",
        "agent_inbox",
        "approval_list",
        "task_list",
        "local_model_list",
        "hygiene_report",
    ]);
    for facade in [
        DashboardFacade::Workspace,
        DashboardFacade::Agent,
        DashboardFacade::Approval,
        DashboardFacade::Task,
        DashboardFacade::Model,
        DashboardFacade::Hygiene,
    ] {
        assert!(!facade.available(&retired));
    }
}
