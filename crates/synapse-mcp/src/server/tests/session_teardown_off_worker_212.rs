//! Regression tests for #212: session teardown must not park a tokio worker
//! thread in blocking process-tree waits, and the spawned-agent stdout summary
//! must not load the whole `stdout.jsonl` into memory.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::failed_process_retain_tests_75::KillOnDrop;
use super::{SessionProcessResource, stdout_summary};
use crate::test_support;

#[tokio::test(flavor = "current_thread")]
async fn must_pass_teardown_of_a_live_process_keeps_the_async_worker_free() {
    let _serial = test_support::lease_serial("teardown_off_worker_212");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    let mut child = KillOnDrop::long_running_ping();
    // No kill-on-close job: teardown has to wait out PROCESS_JOB_CLOSE_WAIT
    // (5 s of polling) before it force-terminates the still-live child.
    service
        .register_session_process_resource(SessionProcessResource {
            session_id: "session-212".to_owned(),
            tool: "act_launch",
            pid: child.id(),
            registered_at_unix_ms: 1,
            resource_id: None,
            launch_target: "ping".to_owned(),
            agent_cli: None,
            process_job: None,
            desktop_lease: None,
            retained_processes: std::collections::BTreeMap::new(),
        })
        .expect("register process resource");

    // On a single-threaded runtime a blocked worker starves this ticker.
    let ticks = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn({
        let ticks = Arc::clone(&ticks);
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
    let started = Instant::now();
    let report = lifecycle
        .teardown_session_with_options_report(
            "session-212",
            "test",
            super::SessionTeardownOptions::explicit_kill(),
        )
        .await;
    let elapsed = started.elapsed();
    ticker.abort();
    let _ = child.kill();
    let _ = child.wait();

    assert!(report.is_ok(), "teardown report: {report:?}");
    assert!(
        elapsed >= Duration::from_secs(3),
        "the process-tree wait should have taken seconds, took {elapsed:?}"
    );
    let observed = ticks.load(Ordering::Relaxed);
    assert!(
        observed >= 25,
        "async worker was starved during teardown: only {observed} ticks in {elapsed:?}"
    );
}

#[test]
fn must_pass_stdout_summary_counts_non_empty_lines_and_reports_last_event_type() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("stdout.jsonl");
    fs::write(
        &path,
        concat!(
            "{\"type\":\"turn.started\"}\n",
            "\n",
            "   \n",
            "{\"item\":{\"type\":\"agent_message\"}}\n",
            "not json at all\n",
        ),
    )
    .expect("write stdout");

    assert_eq!(stdout_summary(&path), (3, Some("agent_message".to_owned())));
}

#[test]
fn must_fail_stdout_summary_of_a_missing_file_is_empty() {
    let temp = TempDir::new().expect("temp dir");

    assert_eq!(stdout_summary(&temp.path().join("absent.jsonl")), (0, None));
}

#[test]
fn must_pass_stdout_summary_of_a_large_file_is_correct() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("stdout.jsonl");
    let mut body = String::new();
    for index in 0..20_000 {
        body.push_str(&format!("{{\"type\":\"event\",\"n\":{index}}}\n"));
    }
    body.push_str("{\"type\":\"turn.completed\"}\n");
    fs::write(&path, &body).expect("write stdout");

    assert_eq!(
        stdout_summary(&path),
        (20_001, Some("turn.completed".to_owned()))
    );
}
