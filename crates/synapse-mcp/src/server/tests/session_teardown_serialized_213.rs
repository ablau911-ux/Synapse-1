//! Regression tests for #213: teardown is serialized per session and an
//! in-progress teardown is not mistaken for a failed one. The 30 s retry and
//! the stale sweep must skip a session whose teardown is still running, and a
//! second teardown of the same session waits instead of racing the first.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use tempfile::TempDir;

use super::failed_process_retain_tests_75::KillOnDrop;
use super::teardown_gate::SessionTeardownGate;
use super::{SessionProcessResource, SessionTeardownOptions};
use crate::test_support;

fn ledger_resource(session_id: &str, pid: u32) -> SessionProcessResource {
    SessionProcessResource {
        session_id: session_id.to_owned(),
        tool: "act_launch",
        pid,
        registered_at_unix_ms: 1,
        resource_id: None,
        launch_target: "ping".to_owned(),
        agent_cli: None,
        process_job: None,
        desktop_lease: None,
        retained_processes: BTreeMap::new(),
    }
}

#[tokio::test]
async fn must_pass_a_second_teardown_of_the_same_session_waits_for_the_first() {
    let gate = SessionTeardownGate::default();
    let first = gate.begin("gate-213").await;
    assert!(gate.is_in_progress("gate-213"));

    let blocked = tokio::time::timeout(Duration::from_millis(150), gate.begin("gate-213")).await;
    assert!(
        blocked.is_err(),
        "second teardown must wait while the first runs"
    );
    // A different session is never blocked by it.
    let other = tokio::time::timeout(Duration::from_millis(150), gate.begin("gate-213-other"))
        .await
        .expect("other session must not wait");
    drop(other);

    drop(first);
    let second = tokio::time::timeout(Duration::from_millis(500), gate.begin("gate-213"))
        .await
        .expect("second teardown proceeds once the first finished");
    drop(second);
    assert!(
        !gate.is_in_progress("gate-213"),
        "gate entry must be cleaned up"
    );
    assert!(!gate.is_in_progress("gate-213-other"));
}

#[tokio::test]
async fn must_fail_a_cancelled_waiter_does_not_leak_the_gate_entry() {
    let gate = SessionTeardownGate::default();
    let first = gate.begin("gate-213-cancel").await;
    let waiter =
        tokio::time::timeout(Duration::from_millis(50), gate.begin("gate-213-cancel")).await;
    assert!(waiter.is_err());
    drop(first);

    assert!(!gate.is_in_progress("gate-213-cancel"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_stale_sweep_skips_a_session_whose_teardown_is_in_progress() {
    let _serial = test_support::lease_serial("teardown_serialized_213");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    // A ledger entry makes the session a stale-sweep candidate.
    service
        .register_session_process_resource(ledger_resource("session-213-sweep", 4_000_000))
        .expect("register ledger entry");
    let active = BTreeSet::new();
    assert!(
        lifecycle
            .stale_session_candidates(&active)
            .contains_key("session-213-sweep")
    );

    let permit = SessionTeardownGate::global()
        .begin("session-213-sweep")
        .await;
    assert!(lifecycle.session_teardown_in_progress("session-213-sweep"));
    assert!(
        !lifecycle
            .stale_session_candidates(&active)
            .contains_key("session-213-sweep"),
        "an in-progress teardown must not be re-selected by the sweep"
    );

    drop(permit);
    assert!(!lifecycle.session_teardown_in_progress("session-213-sweep"));
    assert!(
        lifecycle
            .stale_session_candidates(&active)
            .contains_key("session-213-sweep")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_a_running_teardown_is_visible_as_in_progress() {
    let _serial = test_support::lease_serial("teardown_in_progress_213");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    let mut child = KillOnDrop::long_running_ping();
    service
        .register_session_process_resource(ledger_resource("session-213-run", child.id()))
        .expect("register process resource");

    let teardown = lifecycle.teardown_session_with_options_report(
        "session-213-run",
        "test",
        SessionTeardownOptions::explicit_kill(),
    );
    let observer = async {
        for _ in 0..100 {
            if lifecycle.session_teardown_in_progress("session-213-run") {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    };
    let (report, saw_in_progress) = tokio::join!(teardown, observer);
    let _ = child.kill();
    let _ = child.wait();

    assert!(report.is_ok(), "teardown report: {report:?}");
    assert!(saw_in_progress, "teardown never showed as in progress");
    assert!(!lifecycle.session_teardown_in_progress("session-213-run"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_a_timed_out_teardown_cannot_release_the_session_while_blocking_cleanup_runs() {
    let _serial = test_support::lease_serial("teardown_timeout_213");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    let mut child = KillOnDrop::long_running_ping();
    service
        .register_session_process_resource(ledger_resource("session-213-timeout", child.id()))
        .expect("register process resource");

    // The stale sweep's timeout cancels the teardown future while the blocking
    // process cleanup (a live process waits ~5 s before termination) still runs.
    let cut_short = tokio::time::timeout(
        Duration::from_secs(2),
        lifecycle.teardown_session_with_options_report(
            "session-213-timeout",
            "test",
            SessionTeardownOptions::default(),
        ),
    )
    .await;
    assert!(cut_short.is_err(), "teardown must still be running at 2 s");
    assert!(
        lifecycle.session_teardown_in_progress("session-213-timeout"),
        "the session must stay in progress while blocking cleanup still runs"
    );

    let mut released = false;
    for _ in 0..600 {
        if !lifecycle.session_teardown_in_progress("session-213-timeout") {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(released, "the session must be released once cleanup ends");
}
