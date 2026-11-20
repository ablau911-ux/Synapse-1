//! #59 (reopened): shell kill ownership, executor offload, truthful termination reporting.
//!
//! The accepted fixes (dfa1b054, 5d9d4029, 63cef710) only ever landed on the unmerged
//! `codex/perception-correctness-20260908` branch. On main, cancel and reconcile still pick
//! the process to kill from the PID stored in `status.json`, timeout tree-kills run
//! synchronously on the async executor, and both timeout messages hard-code "the process
//! tree was terminated". Every process spawned here is harmless (`ping -n`) and is killed
//! and waited by a Drop guard.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

const POLL_DEADLINE: Duration = Duration::from_secs(30);
const MAX_EXECUTOR_GAP: Duration = Duration::from_millis(250);

/// A process this test owns that is NOT a Synapse shell job (the "foreign" victim).
struct ForeignProcess(Child);

impl ForeignProcess {
    fn spawn() -> Self {
        let child = Command::new("ping")
            .args(["-n", "120", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("spawn foreign ping: {error}"));
        Self(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn still_running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
}

impl Drop for ForeignProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_params(job_id: &str, timeout_ms: Option<u64>) -> ActRunShellStartParams {
    ActRunShellStartParams {
        command: "cmd.exe".to_owned(),
        args: vec![
            "/d".to_owned(),
            "/c".to_owned(),
            "ping -n 60 127.0.0.1".to_owned(),
        ],
        working_dir: None,
        env: BTreeMap::new(),
        timeout_ms,
        job_id: Some(job_id.to_owned()),
    }
}

fn authorization(params: &ActRunShellStartParams) -> RunShellAuthorization {
    RunShellAuthorization {
        command_line: shell_command_line_from_parts(&params.command, &params.args),
        matched_pattern: "issue59-synthetic".to_owned(),
    }
}

/// Writes a live `running` status whose stored PID belongs to `foreign` — the crash-stale /
/// recycled-PID record from the issue's reproduction — without any daemon-owned job handle.
fn write_stale_running_status(job_id: &str, foreign: &ForeignProcess) {
    let params = start_params(job_id, None);
    let paths =
        shell_job_paths_for_id(None, job_id).unwrap_or_else(|error| panic!("job paths: {error:?}"));
    std::fs::create_dir_all(&paths.job_dir)
        .unwrap_or_else(|error| panic!("create job dir: {error}"));
    // A real job creates both logs before publishing its running status.
    for path in [&paths.stdout_path, &paths.stderr_path] {
        std::fs::write(path, b"").unwrap_or_else(|error| panic!("create job log: {error}"));
    }
    let sha = run_shell_start_request_sha256(&params)
        .unwrap_or_else(|error| panic!("request sha: {error:?}"));
    let status = shell_job_status_record(
        job_id,
        "running",
        &params,
        &paths,
        &sha,
        &authorization(&params),
        chrono::Utc::now().to_rfc3339(),
        Some(foreign.pid()),
        None,
    );
    write_shell_job_status(&paths.status_path, &status)
        .unwrap_or_else(|error| panic!("write stale status: {error:?}"));
    let readback = read_shell_job_status(&paths.status_path, job_id)
        .unwrap_or_else(|error| panic!("precondition readback: {error:?}"));
    assert_eq!(
        readback.status, "running",
        "precondition: stale record is live"
    );
    assert_eq!(
        readback.pid,
        Some(foreign.pid()),
        "precondition: PID aliases the foreign process"
    );
}

fn job_id_params(job_id: &str) -> ActRunShellJobIdParams {
    ActRunShellJobIdParams {
        job_id: job_id.to_owned(),
    }
}

fn status_of(job_id: &str) -> ActRunShellStatusResponse {
    shell_job_status(
        &ActRunShellStatusParams {
            job_id: job_id.to_owned(),
            tail_bytes: 1024,
        },
        None,
    )
    .unwrap_or_else(|error| panic!("status {job_id}: {error:?}"))
}

#[test]
fn must_fail_cancel_never_kills_a_foreign_process_named_by_a_stale_pid_59() {
    let _root = ShellJobRootGuard::new();
    let mut foreign = ForeignProcess::spawn();
    assert!(
        foreign.still_running(),
        "precondition: foreign process alive"
    );
    write_stale_running_status("issue59-stale-cancel", &foreign);

    let response = cancel_shell_job(&job_id_params("issue59-stale-cancel"), None)
        .unwrap_or_else(|error| panic!("cancel: {error:?}"));

    assert!(
        foreign.still_running(),
        "cancel must not terminate a process this daemon does not own"
    );
    assert!(!response.termination_attempted);
    assert_eq!(response.termination_status, "job_handle_unavailable");
    assert!(response.remaining_process_ids.is_empty());
}

#[test]
fn must_fail_status_of_a_stale_pid_job_is_ownership_unverified_not_running_59() {
    let _root = ShellJobRootGuard::new();
    let mut foreign = ForeignProcess::spawn();
    write_stale_running_status("issue59-stale-status", &foreign);

    let response = status_of("issue59-stale-status");

    assert!(foreign.still_running(), "status readback must never kill");
    assert_eq!(response.job.status, "ownership_unverified");
    assert_eq!(response.job.completed_at, None);
    assert!(!response.running);
}

/// Polls the physical `status.json` only (no reconcile: reconcile itself sleeps
/// synchronously and would pollute the executor-gap measurement).
async fn wait_terminal(job_id: &str) -> ActRunShellJobStatus {
    let paths =
        shell_job_paths_for_id(None, job_id).unwrap_or_else(|error| panic!("job paths: {error:?}"));
    let started = Instant::now();
    loop {
        let job = read_shell_job_status(&paths.status_path, job_id)
            .unwrap_or_else(|error| panic!("status file {job_id}: {error:?}"));
        if shell_job_terminal_status(&job.status) {
            return job;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "job {job_id} never finished: {job:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_pid_gone(pid: u32) {
    let started = Instant::now();
    while process_exists(pid) {
        assert!(started.elapsed() < POLL_DEADLINE, "pid {pid} still alive");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn start_job(job_id: &str, timeout_ms: Option<u64>) -> ActRunShellJobStatus {
    let params = start_params(job_id, timeout_ms);
    let auth = authorization(&params);
    let started = start_authorized_shell_job(params, &auth, None)
        .unwrap_or_else(|error| panic!("start {job_id}: {error:?}"));
    assert_eq!(started.job.status, "running", "precondition: job started");
    assert!(started.job.pid.is_some(), "precondition: job has a pid");
    started.job
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_cancel_of_a_live_owned_job_terminates_its_tree_59() {
    let _root = ShellJobRootGuard::new();
    let job = start_job("issue59-owned-cancel", None);
    let root_pid = job.pid.unwrap_or_default();
    let tree = owned_process_tree_ids(root_pid);
    assert!(process_exists(root_pid), "precondition: owned root alive");

    let response = cancel_shell_job(&job_id_params("issue59-owned-cancel"), None)
        .unwrap_or_else(|error| panic!("cancel: {error:?}"));

    assert!(response.cancel_requested);
    assert!(response.termination_attempted);
    assert_eq!(response.termination_status, "terminated");
    assert_eq!(response.remaining_process_ids, Vec::<u32>::new());
    for pid in tree {
        wait_pid_gone(pid).await;
    }
    assert_eq!(
        wait_terminal("issue59-owned-cancel").await.status,
        "cancelled"
    );
    assert_eq!(status_of("issue59-owned-cancel").job.status, "cancelled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_durable_timeout_message_reports_the_real_termination_status_59() {
    let _root = ShellJobRootGuard::new();
    let job = start_job("issue59-durable-timeout", Some(500));
    let root_pid = job.pid.unwrap_or_default();

    let job = wait_terminal("issue59-durable-timeout").await;
    wait_pid_gone(root_pid).await;

    assert_eq!(job.status, "timed_out");
    assert!(job.timed_out);
    assert_eq!(
        job.error_code.as_deref(),
        Some(error_codes::ACTION_BUDGET_EXPIRED)
    );
    let message = job.error_message.unwrap_or_default();
    assert!(
        message.contains("process-tree termination status: terminated"),
        "message must carry the readback status: {message}"
    );
    assert!(
        !message.contains("the process tree was terminated"),
        "{message}"
    );
}

fn inline_params(timeout_ms: u64) -> ActRunShellParams {
    serde_json::from_value(json!({
        "command": "cmd.exe",
        "args": ["/d", "/c", "ping -n 60 127.0.0.1"],
        "timeout_ms": timeout_ms,
        "execution_mode": "inline",
    }))
    .unwrap_or_else(|error| panic!("inline params: {error}"))
}

async fn run_inline_timeout() -> ActRunShellResponse {
    let params = inline_params(400);
    let auth = RunShellAuthorization {
        command_line: shell_command_line(&params),
        matched_pattern: "issue59-synthetic".to_owned(),
    };
    run_authorized_shell(params, &auth, 60_000, None)
        .await
        .unwrap_or_else(|error| panic!("inline run: {error:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_inline_timeout_message_reports_the_real_termination_status_59() {
    let response = run_inline_timeout().await;

    assert!(response.timed_out);
    assert_eq!(response.exit_code, None);
    assert_eq!(
        response.error_code.as_deref(),
        Some(error_codes::ACTION_BUDGET_EXPIRED)
    );
    let message = response.error_message.unwrap_or_default();
    assert!(
        message.contains("process-tree termination status: terminated"),
        "message must carry the readback status: {message}"
    );
    assert!(
        !message.contains("the process tree was terminated"),
        "{message}"
    );
}

/// Runs `work` on the current-thread executor next to a 10 ms ticker and returns the
/// largest gap between ticks: synchronous kill work on the executor shows up as a gap.
async fn max_executor_gap<F: std::future::Future>(work: F) -> (F::Output, Duration) {
    let ticks = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let ticker_ticks = Arc::clone(&ticks);
    let ticker = tokio::spawn(async move {
        loop {
            if let Ok(mut ticks) = ticker_ticks.lock() {
                ticks.push(Instant::now());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let output = work.await;
    ticker.abort();
    let ticks = ticks.lock().map(|ticks| ticks.clone()).unwrap_or_default();
    let gap = ticks
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .max()
        .unwrap_or_default();
    (output, gap)
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_inline_timeout_tree_kill_does_not_stall_the_executor_59() {
    let (response, gap) = max_executor_gap(run_inline_timeout()).await;

    assert!(response.timed_out, "precondition: the timeout path ran");
    assert!(
        gap < MAX_EXECUTOR_GAP,
        "tree termination blocked the async executor for {gap:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_durable_timeout_tree_kill_does_not_stall_the_executor_59() {
    let _root = ShellJobRootGuard::new();
    let job = start_job("issue59-durable-stall", Some(300));
    let (finished, gap) = max_executor_gap(wait_terminal("issue59-durable-stall")).await;
    wait_pid_gone(job.pid.unwrap_or_default()).await;

    assert_eq!(
        finished.status, "timed_out",
        "precondition: the timeout path ran"
    );
    assert!(
        gap < MAX_EXECUTOR_GAP,
        "durable timeout tree termination blocked the async executor for {gap:?}"
    );
}

#[test]
fn must_fail_guard_no_hard_coded_termination_claim_remains_in_m4_59() {
    let source = include_str!("../m4.rs");
    assert_eq!(
        source.matches("the process tree was terminated").count(),
        0,
        "timeout messages must report the real termination readback"
    );
}
