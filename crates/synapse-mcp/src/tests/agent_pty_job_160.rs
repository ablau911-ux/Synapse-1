//! #160: an `act_spawn_agent` wrapper (spawned through ConPTY) must be inside its
//! kill-on-close job object before it executes a single instruction — the same
//! guarantee #40 gave shell jobs and `act_launch`.
//!
//! NEEDS-SEAM (the fixer adds it; these tests are written against it):
//! `crate::server::terminal_capture::capture::spawn_capture_to_asciicast_with_job`
//! creates the pseudoconsole child suspended (or with PROC_THREAD_ATTRIBUTE_JOB_LIST),
//! calls `assign(pid, "act_spawn_agent", Some(resource_id))` while the child has run
//! nothing, resumes it only after `assign` returned `Ok`, and returns the job. On an
//! assignment error it terminates the never-resumed child and fails with an error whose
//! text starts with `PTY_JOB_ASSIGN_FAILED`. Children here are harmless `cmd.exe` +
//! `ping` and die with their job when the test drops it.

#![cfg(windows)]

use std::time::{Duration, Instant};

use super::job_assign_failure_40_tests::{
    AssignmentObservation, EXIT_TIMEOUT, MARKER_TIMEOUT, UNSUSPENDED_RUN_WINDOW,
    synthetic_assignment_failure,
};
use super::*;
use crate::server::terminal_capture::capture::{
    CaptureArtifacts, CaptureSpec, spawn_capture_to_asciicast_with_job,
};

const MARKER: &str = "issue160-pty-child-ran.txt";

fn spec(dir: &Path) -> CaptureSpec {
    CaptureSpec {
        live_key: None,
        program: "cmd.exe".to_owned(),
        args: vec![
            "/d".to_owned(),
            "/c".to_owned(),
            format!("echo ran>{MARKER} & ping -n 120 127.0.0.1 >nul"),
        ],
        cwd: Some(dir.to_path_buf()),
        env: std::env::vars().collect(),
        cols: 80,
        rows: 25,
        started_unix_secs: 0,
        title: Some("issue160".to_owned()),
    }
}

fn artifacts(dir: &Path) -> CaptureArtifacts {
    CaptureArtifacts {
        asciicast_path: dir.join("terminal.cast"),
        status_path: dir.join("terminal-status.json"),
        final_screen_path: dir.join("terminal-final.txt"),
        input_audit_path: dir.join("terminal-input.jsonl"),
    }
}

fn wait_for(path: &Path, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn must_fail_agent_pty_child_runs_only_after_it_is_inside_its_job_160() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("dir: {error}"));
    let marker = dir.path().join(MARKER);
    let mut observed = None;
    let mut ran_before_assign = None;

    let (spawned, job) = spawn_capture_to_asciicast_with_job(
        spec(dir.path()),
        artifacts(dir.path()),
        "agent-spawn-issue160",
        |pid, tool_name, resource_id| {
            observed = Some(AssignmentObservation::capture(pid));
            std::thread::sleep(UNSUSPENDED_RUN_WINDOW);
            ran_before_assign = Some(marker.exists());
            assign_owned_process_job(pid, tool_name, resource_id)
        },
    )
    .unwrap_or_else(|error| panic!("spawn must succeed: {error:#}"));

    let observed = observed.unwrap_or_else(|| panic!("assign must run"));
    observed.assert_fully_suspended();
    assert_eq!(
        ran_before_assign,
        Some(false),
        "the child ran before its job existed"
    );
    assert_eq!(spawned.process_id, observed.pid);
    assert!(
        observed.pinned.is_in_job(&job),
        "the child must be a member of its job"
    );
    assert!(
        wait_for(&marker, MARKER_TIMEOUT),
        "the child must be resumed after assignment"
    );
    drop(job); // kill-on-close ends the ping
}

#[test]
fn must_fail_assignment_failure_kills_the_never_resumed_pty_child_160() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("dir: {error}"));
    let marker = dir.path().join(MARKER);
    let mut observed = None;

    let error = spawn_capture_to_asciicast_with_job(
        spec(dir.path()),
        artifacts(dir.path()),
        "agent-spawn-issue160-fail",
        |pid, _tool_name, _resource_id| {
            observed = Some(AssignmentObservation::capture(pid));
            Err(synthetic_assignment_failure())
        },
    )
    .err()
    .unwrap_or_else(|| panic!("an assignment failure must fail the spawn"));

    assert!(
        format!("{error:#}").starts_with("PTY_JOB_ASSIGN_FAILED"),
        "{error:#}"
    );
    let observed = observed.unwrap_or_else(|| panic!("assign must run"));
    observed.assert_fully_suspended();
    let started = Instant::now();
    while observed.pinned.exit_code().is_none() && started.elapsed() < EXIT_TIMEOUT {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        observed.pinned.exit_code(),
        Some(1),
        "the child must be terminated"
    );
    assert!(
        !marker.exists(),
        "the unassigned child must never run its command"
    );
}

/// Guard: the production spawn must take its job from the seam, not assign after launch.
#[test]
fn must_fail_guard_act_spawn_agent_does_not_assign_the_job_after_launch_160() {
    let source = include_str!("../server/m4_tools.rs").replace('\r', "");
    let launched = source
        .find("launch_agent_spawn_with_terminal_capture(\n            &self.m4_config")
        .unwrap_or_else(|| panic!("act_spawn_agent launch call not found"));
    let after = &source[launched..];
    let window = &after[..after.len().min(4_000)];
    assert!(
        !window.contains("assign_owned_process_job(\n            launch_response.pid"),
        "the wrapper is assigned to its job only after it already ran (#160)"
    );
}
