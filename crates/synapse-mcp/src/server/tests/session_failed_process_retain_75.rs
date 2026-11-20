//! #75: a failed process cleanup must keep ownership of every resource whose
//! teardown failed, so a retry still sees it and only a verified exit (process
//! gone) lets the teardown report success and clear `cleanup_failed`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    path::Path,
    process::{Child, Command, Stdio},
};

use tokio_util::sync::CancellationToken;

use super::{SessionProcessResource, SessionTeardownOptions, SynapseService};
use crate::{
    m2::M2ServiceConfig,
    m3::M3ServiceConfig,
    m4::{self, M4ServiceConfig, OwnedProcessTerminationReadback},
};

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

/// Owns a child process and kills + reaps it on drop, so a failed assertion
/// mid-test never leaks the 120 s `ping` child. Derefs to the `Child`.
/// Shared by the other session-teardown test files.
pub(super) struct KillOnDrop(pub(super) Child);

impl KillOnDrop {
    /// Spawns a quiet `ping -n 120` child that lives about two minutes.
    pub(super) fn long_running_ping() -> Self {
        Self(
            Command::new("ping")
                .args(["-n", "120", "127.0.0.1"])
                .stdout(Stdio::null())
                .spawn()
                .expect("spawn ping"),
        )
    }
}

impl std::ops::Deref for KillOnDrop {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn long_running_child() -> KillOnDrop {
    KillOnDrop::long_running_ping()
}

/// Teardown stub that fails to kill anything and reports the root as surviving.
fn failing_terminator(pid: u32) -> OwnedProcessTerminationReadback {
    OwnedProcessTerminationReadback {
        pid,
        process_ids: vec![pid],
        live_process_ids_before: vec![pid],
        attempted: true,
        status: "stub_termination_failed".to_owned(),
        remaining_process_ids: vec![pid],
        quiesce_failures: Vec::new(),
    }
}

fn resource(session_id: &str, pid: u32) -> SessionProcessResource {
    SessionProcessResource {
        session_id: session_id.to_owned(),
        tool: "act_spawn_agent",
        pid,
        registered_at_unix_ms: 1,
        resource_id: Some(format!("res-{pid}")),
        launch_target: "ping.exe".to_owned(),
        agent_cli: None,
        process_job: None,
        desktop_lease: None,
        retained_processes: BTreeMap::new(),
    }
}

#[test]
fn must_fail_failed_process_cleanup_retains_resource_until_exit_is_verified() {
    let temp = tempfile::tempdir().expect("tempdir");
    let svc = service(temp.path());
    let lifecycle = svc.session_lifecycle_state().expect("lifecycle");
    let mut child = long_running_child();
    let pid = child.id();
    svc.register_session_process_resource(resource("s75", pid))
        .expect("register");
    let options = SessionTeardownOptions::explicit_kill();
    let ledger_holds = |lifecycle: &super::SessionLifecycleState| {
        lifecycle
            .session_processes
            .lock()
            .expect("ledger")
            .get("s75")
            .is_some_and(|resources| resources.contains_key(&pid))
    };

    // First cleanup fails to terminate the child: ownership must be kept.
    let first = lifecycle.cleanup_owned_processes_with("s75", options, &failing_terminator);
    assert_eq!(first.failed, 1, "failed termination is reported");
    assert!(ledger_holds(&lifecycle), "failed resource must be retained");

    // Retry still sees the child and still fails; it must not report success.
    let retry = lifecycle.cleanup_owned_processes_with("s75", options, &failing_terminator);
    assert_eq!(retry.owned_before, 1, "retry sees the retained resource");
    assert_eq!(retry.failed, 1, "retry must not clear a surviving child");
    assert!(ledger_holds(&lifecycle), "still retained while child lives");

    // Child exits; retry verifies it is gone and clears the ledger.
    child.kill().expect("kill child");
    child.wait().expect("wait child");
    let verified =
        lifecycle.cleanup_owned_processes_with("s75", options, &m4::terminate_owned_process_tree);
    assert_eq!(verified.owned_before, 1);
    assert_eq!(verified.failed, 0, "verified exit clears the failure");
    assert_eq!(verified.terminated, 1);
    assert!(
        !ledger_holds(&lifecycle),
        "ledger empty after verified exit"
    );
}

#[test]
fn must_pass_retained_orphan_survivor_is_found_after_root_exit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let svc = service(temp.path());
    let lifecycle = svc.session_lifecycle_state().expect("lifecycle");
    let mut root = long_running_child();
    let mut orphan = long_running_child();
    let (root_pid, orphan_pid) = (root.id(), orphan.id());
    svc.register_session_process_resource(resource("s75o", root_pid))
        .expect("register");
    let options = SessionTeardownOptions::explicit_kill();

    // Terminator reports the orphan (not the root) as the surviving process.
    let fail_orphan = move |pid: u32| OwnedProcessTerminationReadback {
        remaining_process_ids: vec![orphan_pid],
        ..failing_terminator(pid)
    };
    let first = lifecycle.cleanup_owned_processes_with("s75o", options, &fail_orphan);
    assert_eq!(first.failed, 1);

    // Root dies; only the retained orphan keeps the retry failing.
    root.kill().expect("kill root");
    root.wait().expect("wait root");
    let retry =
        lifecycle.cleanup_owned_processes_with("s75o", options, &m4::terminate_owned_process_tree);
    assert_eq!(retry.failed, 0, "real terminator kills the retained orphan");
    assert!(
        orphan.try_wait().expect("try_wait").is_some(),
        "retained orphan was terminated and verified gone"
    );
}
