//! #75: a spawned agent whose OS process is gone must be reaped even while its
//! HTTP session is still in the manager map (abrupt death skips DELETE, so the
//! session stays "active" for the 24 h keep-alive).
#![cfg(windows)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::BTreeSet,
    num::NonZeroUsize,
    path::Path,
    process::{Child, Command},
};

use tokio_util::sync::CancellationToken;

use super::{SPAWNED_AGENT_PROCESS_EXITED_REASON, SynapseService};
use crate::server::session_registry::SpawnedAgentRead;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

fn service(path: &Path) -> SynapseService {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(path.join("db")),
            Some(path.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).expect("nonzero"),
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
    .expect("service")
}

/// A child that has exited. The returned `Child` MUST be kept alive by the
/// caller for the whole test: it owns the process handle, and Windows never
/// reassigns a pid while any handle to the exited process is open, so the pid
/// cannot be reused by a live process before the liveness probe runs.
fn exited_child() -> Child {
    let mut child = Command::new("cmd")
        .args(["/C", "exit 0"])
        .spawn()
        .expect("spawn");
    child.wait().expect("wait");
    child
}

fn spawned(pid: u32) -> SpawnedAgentRead {
    SpawnedAgentRead {
        spawn_id: format!("spawn-{pid}"),
        cli: "codex".to_owned(),
        launcher_process_id: pid,
        agent_process_id: Some(pid),
        started_by_session_id: Some("parent".to_owned()),
        launched_at_unix_ms: 1,
        launch_target: "pwsh.exe".to_owned(),
        log_dir: r"C:\nonexistent\issue75".to_owned(),
        template_id: None,
        template_version: None,
        control: None,
    }
}

#[test]
fn must_pass_dead_spawned_owner_with_active_http_session_is_reaped() {
    let temp = tempfile::tempdir().expect("tempdir");
    let svc = service(temp.path());
    let lifecycle = svc.session_lifecycle_state().expect("lifecycle");
    #[expect(
        clippy::zombie_processes,
        reason = "exited_child() already waited; the handle is kept so the PID cannot be reused"
    )]
    let dead_child = exited_child();
    let dead = dead_child.id();
    let live = std::process::id();
    {
        let mut registry = lifecycle.session_registry.lock().expect("registry");
        registry.record_spawned_agent("dead-active", spawned(dead), 10);
        registry.record_spawned_agent("live-active", spawned(live), 10);
        registry.record_spawned_agent("dead-cleanup-failed", spawned(dead), 10);
        registry.record_cleanup_result("dead-cleanup-failed", false);
    }
    let active: BTreeSet<String> = [
        "dead-active",
        "live-active",
        "dead-cleanup-failed",
        "plain-active",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();

    let candidates = lifecycle.stale_session_candidates(&active);

    assert_eq!(
        candidates.get("dead-active"),
        Some(&SPAWNED_AGENT_PROCESS_EXITED_REASON),
        "OS-dead spawned owner must be reaped despite HTTP keep-alive membership"
    );
    assert!(
        !candidates.contains_key("live-active"),
        "live spawned owner stays protected"
    );
    assert!(
        !candidates.contains_key("dead-cleanup-failed"),
        "failed-cleanup ownership is retained for the retry path"
    );
    assert!(
        !candidates.contains_key("plain-active"),
        "non-spawn active session untouched"
    );
}

#[tokio::test]
async fn must_pass_dead_spawned_owner_with_failed_cleanup_and_live_http_is_retried() {
    let temp = tempfile::tempdir().expect("tempdir");
    let svc = service(temp.path());
    let lifecycle = svc.session_lifecycle_state().expect("lifecycle");
    #[expect(
        clippy::zombie_processes,
        reason = "exited_child() already waited; the handle is kept so the PID cannot be reused"
    )]
    let dead_child = exited_child();
    let dead = dead_child.id();
    let live = std::process::id();
    {
        let mut registry = lifecycle.session_registry.lock().expect("registry");
        registry.record_spawned_agent("dead-failed", spawned(dead), 10);
        registry.record_cleanup_result("dead-failed", false);
        registry.record_spawned_agent("live-failed", spawned(live), 10);
        registry.record_cleanup_result("live-failed", false);
    }
    let (retries, dead_owner_retries, _protected) =
        lifecycle.failed_cleanup_retry_candidates().await;
    assert!(retries.contains("dead-failed"));
    assert!(dead_owner_retries.contains("dead-failed"));
    assert!(
        !retries.contains("live-failed"),
        "live owner is never retried"
    );

    // HTTP keep-alive membership must not suppress a confirmed-dead owner's retry.
    let http_active: BTreeSet<String> = ["dead-failed".to_owned(), "plain".to_owned()].into();
    let mut with_plain = retries.clone();
    with_plain.insert("plain".to_owned());
    let teardown = crate::server::session_lifecycle::retry_teardown_ids(
        &with_plain,
        &dead_owner_retries,
        &http_active,
    );
    assert!(
        teardown.contains("dead-failed"),
        "dead owner retried despite HTTP-live"
    );
    assert!(
        !teardown.contains("plain"),
        "non-spawn HTTP-live retry stays suppressed"
    );
}

#[test]
fn must_fail_live_spawned_owner_without_http_session_is_never_reaped() {
    let temp = tempfile::tempdir().expect("tempdir");
    let svc = service(temp.path());
    let lifecycle = svc.session_lifecycle_state().expect("lifecycle");
    #[expect(
        clippy::zombie_processes,
        reason = "exited_child() already waited; the handle is kept so the PID cannot be reused"
    )]
    let dead_child = exited_child();
    let dead = dead_child.id();
    let live = std::process::id();
    {
        let mut registry = lifecycle.session_registry.lock().expect("registry");
        // Last seen at t=10 ms: both sessions are long MCP-idle.
        registry.record_spawned_agent("live-idle", spawned(live), 10);
        registry.record_spawned_agent("dead-idle", spawned(dead), 10);
    }
    // Idle-evicted from the HTTP manager: no active sessions at all.
    let candidates = lifecycle.stale_session_candidates(&BTreeSet::new());

    assert!(
        !candidates.contains_key("live-idle"),
        "a running spawned agent must never be reaped for MCP idleness"
    );
    assert_eq!(
        candidates.get("dead-idle"),
        Some(&SPAWNED_AGENT_PROCESS_EXITED_REASON),
        "only the process probe decides a spawned agent's liveness"
    );
}
