//! #40 regression: an `act_launch` child never runs outside its kill-on-close Windows job.
//!
//! A session launch creates the child suspended, places it in its job, and only then
//! resumes it. A failed assignment must terminate the still-suspended child before it
//! executes anything and return a precise error. The assignment step is injected through
//! the `spawn_launch_child_with` seam, so the failure path runs without admin rights.

#![cfg(windows)]

use super::job_assign_failure_40_tests::{
    AssignmentObservation, EXIT_TIMEOUT, MARKER_TIMEOUT, PinnedProcess, UNSUSPENDED_RUN_WINDOW,
    synthetic_assignment_failure,
};
use super::*;

/// File the child creates in its working directory once it runs.
const MARKER: &str = "issue40-launch-child-ran.txt";

fn launch_params_for(dir: &Path, script: &str) -> ActLaunchParams {
    ActLaunchParams {
        target: "cmd.exe".to_owned(),
        args: vec!["/d".to_owned(), "/c".to_owned(), script.to_owned()],
        working_dir: Some(dir.to_string_lossy().into_owned()),
        env: BTreeMap::new(),
        wait_for_window_title_regex: None,
        timeout_ms: 10_000,
        idempotency_key: None,
        cdp_debug: Some(false),
        force_renderer_accessibility: None,
        windows_console_window_state: Some(LaunchWindowState::Hidden),
        desktop: None,
    }
}

fn wait_for_file(path: &Path) -> bool {
    let started = Instant::now();
    while started.elapsed() < MARKER_TIMEOUT {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn wait_for_exit(pinned: &PinnedProcess) -> Option<u32> {
    let started = Instant::now();
    while started.elapsed() < EXIT_TIMEOUT {
        if let Some(code) = pinned.exit_code() {
            return Some(code);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

#[test]
fn must_fail_launch_assignment_failure_kills_suspended_child_40() {
    let dir = tempfile::tempdir().expect("working dir");
    let params = launch_params_for(dir.path(), &format!("echo ran>{MARKER}"));

    let mut observed = None;
    let error = spawn_launch_child_with(&params, None, true, |pid, _tool_name, _resource_id| {
        observed = Some(AssignmentObservation::capture(pid));
        std::thread::sleep(UNSUSPENDED_RUN_WINDOW);
        Err(synthetic_assignment_failure())
    })
    .expect_err("an injected assignment failure must fail the launch");
    let observed = observed.expect("the assignment step must run");
    observed.assert_fully_suspended();

    let data = error.data.as_ref().expect("the error must carry readback");
    assert_eq!(data["reason"], "process_job_assign_failed");
    assert_eq!(data["pid"], observed.pid);
    assert_eq!(data["source_error"], "synthetic job assignment failure");
    assert_eq!(
        data["source_error_data"]["reason"],
        "synthetic_job_assign_failure"
    );
    assert_eq!(data["child_resumed"], false);
    assert_eq!(data["child_reaped"], true, "{data}");
    assert_eq!(data["child_exit_code"], 1, "{data}");
    assert!(
        error.message.contains("act_launch"),
        "the error must name the tool: {}",
        error.message
    );

    assert_eq!(
        observed.pinned.exit_code(),
        Some(1),
        "the unassigned child must be terminated"
    );
    let leftovers = fs::read_dir(dir.path())
        .expect("working dir readback")
        .count();
    assert_eq!(
        leftovers, 0,
        "the unassigned child must never run its command"
    );
}

#[test]
fn must_pass_launch_child_stays_suspended_until_inside_its_job_40() {
    let dir = tempfile::tempdir().expect("working dir");
    let script = format!("echo ran>{MARKER} & ping -n 120 127.0.0.1 >nul");
    let params = launch_params_for(dir.path(), &script);

    let mut observed = None;
    let spawned = spawn_launch_child_with(&params, None, true, |pid, tool_name, resource_id| {
        observed = Some(AssignmentObservation::capture(pid));
        assign_owned_process_job(pid, tool_name, resource_id)
    })
    .expect("the launched child must start inside its job");
    let observed = observed.expect("the assignment step must run");
    observed.assert_fully_suspended();
    assert_eq!(spawned.pid, observed.pid);
    let process_job = spawned
        .process_job
        .expect("a session launch must return the job it was confined to");
    assert!(
        observed.pinned.is_in_job(&process_job),
        "the child must be a member of its own job"
    );
    assert!(
        wait_for_file(&dir.path().join(MARKER)),
        "the child must be resumed and run its command"
    );
    assert_eq!(
        observed.pinned.exit_code(),
        None,
        "the child must still be running"
    );

    // Closing the kill-on-close job terminates its members: physical proof of membership.
    drop(process_job);
    assert!(
        wait_for_exit(&observed.pinned).is_some(),
        "closing the job must end the child"
    );
}

#[test]
fn must_pass_launch_without_session_runs_immediately_and_has_no_job_40() {
    let dir = tempfile::tempdir().expect("working dir");
    let params = launch_params_for(dir.path(), &format!("echo ran>{MARKER}"));

    let spawned =
        spawn_launch_child_with(&params, None, false, |_pid, _tool_name, _resource_id| {
            Err(synthetic_assignment_failure())
        })
        .expect("a sessionless launch never assigns a job");
    assert!(spawned.process_job.is_none());
    assert!(
        wait_for_file(&dir.path().join(MARKER)),
        "a sessionless launch must run its command"
    );
}
