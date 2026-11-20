//! Regression tests for #25: a spawn whose wrapper already recorded a terminal
//! failure is reported promptly; partial, stale, foreign or unrelated artifacts
//! never shorten the readiness deadline.

use super::*;

const SPAWN_ID: &str = "agent-spawn-terminal-test";
const LAUNCHER_PID: u32 = 4242;

fn files_in(root: &std::path::Path) -> AgentSpawnFiles {
    let dir = root.join(SPAWN_ID);
    fs::create_dir_all(&dir).expect("spawn dir");
    AgentSpawnFiles {
        prompt_path: dir.join("prompt.txt"),
        stdout_path: dir.join("stdout.jsonl"),
        stderr_path: dir.join("stderr.log"),
        final_message_path: dir.join("final-message.txt"),
        completion_status_path: dir.join("completion-status.json"),
        task_started_path: dir.join("task-started.json"),
        task_started_script_path: dir.join("write-task-started.ps1"),
        log_dir: dir,
        debug_path: None,
        mcp_config_path: None,
        hook_settings_path: None,
        notify_script_path: None,
        codex_app_server_runner_path: None,
        codex_app_server_control_path: None,
        codex_app_server_events_path: None,
        codex_app_server_stdout_path: None,
        codex_app_server_stderr_path: None,
        local_model_runner_path: None,
    }
}

fn completion(files: &AgentSpawnFiles, status: &str, at: u64) -> Value {
    json!({
        "schema_version": 1,
        "spawn_id": SPAWN_ID,
        "cli": "codex",
        "wrapper_process_id": LAUNCHER_PID,
        "task_started_path": files.task_started_path.display().to_string(),
        "wrapper_started_at_unix_ms": at,
        "completed_at_unix_ms": at,
        "status": status,
    })
}

fn read(files: &AgentSpawnFiles, launched_at: u64) -> Option<Value> {
    read_agent_spawn_terminal_failure(
        files,
        ActSpawnAgentCli::Codex,
        SPAWN_ID,
        launched_at,
        LAUNCHER_PID,
    )
}

#[test]
fn must_pass_a_fresh_failed_completion_artifact_is_terminal_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    let artifact = completion(&files, "failed", launched + 100);
    fs::write(&files.completion_status_path, artifact.to_string()).expect("write");
    let evidence = read(&files, launched).expect("terminal evidence");
    assert_eq!(evidence["reason"], "spawn_terminal_failure");
    assert_eq!(evidence["source"], "completion_status");
}

#[test]
fn must_fail_missing_partial_stale_or_foreign_artifacts_are_not_terminal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    assert!(read(&files, launched).is_none(), "no artifact");

    fs::write(&files.completion_status_path, "{\"schema_version\":").expect("partial");
    assert!(
        read(&files, launched).is_none(),
        "partially written artifact"
    );

    let stale = completion(&files, "failed", launched.saturating_sub(60_000));
    fs::write(&files.completion_status_path, stale.to_string()).expect("stale");
    assert!(
        read(&files, launched).is_none(),
        "artifact older than the launch"
    );

    let mut foreign = completion(&files, "failed", launched + 100);
    foreign["wrapper_process_id"] = json!(LAUNCHER_PID + 1);
    fs::write(&files.completion_status_path, foreign.to_string()).expect("foreign");
    assert!(read(&files, launched).is_none(), "different wrapper pid");

    let success = completion(&files, "completed", launched + 100);
    fs::write(&files.completion_status_path, success.to_string()).expect("success");
    assert!(read(&files, launched).is_none(), "a non-failure status");
}

#[test]
fn must_pass_artifact_stamped_exactly_at_the_launch_instant_is_terminal() {
    // Freshness is inclusive: `at >= launched_at_unix_ms`.
    let temp = tempfile::tempdir().expect("tempdir");
    let files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    let artifact = completion(&files, "failed", launched);
    fs::write(&files.completion_status_path, artifact.to_string()).expect("write");
    let evidence = read(&files, launched).expect("an artifact at the launch instant is fresh");
    assert_eq!(evidence["reason"], "spawn_terminal_failure");
    assert_eq!(evidence["source"], "completion_status");
}

#[test]
fn must_fail_artifact_one_millisecond_before_launch_is_not_terminal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    let artifact = completion(&files, "failed", launched - 1);
    fs::write(&files.completion_status_path, artifact.to_string()).expect("write");
    assert!(
        read(&files, launched).is_none(),
        "an artifact stamped 1 ms before the launch is stale"
    );
}

#[test]
fn must_fail_artifact_stamped_in_the_future_is_not_terminal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    let artifact = completion(&files, "failed", unix_time_ms_now() + 3_600_000);
    fs::write(&files.completion_status_path, artifact.to_string()).expect("write");
    assert!(
        read(&files, launched).is_none(),
        "an artifact stamped after now cannot be genuine evidence"
    );
}

#[test]
fn must_fail_a_log_dir_that_is_not_named_for_the_spawn_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    let artifact = completion(&files, "failed", launched + 100);
    fs::write(&files.completion_status_path, artifact.to_string()).expect("write");
    files.log_dir = temp.path().join("some-other-dir");
    assert!(read(&files, launched).is_none());
}

#[test]
fn must_pass_wrapper_error_and_missing_final_response_are_terminal_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    for status in ["wrapper_error", "missing_final_response"] {
        let artifact = completion(&files, status, launched + 100);
        fs::write(&files.completion_status_path, artifact.to_string()).expect("write");
        let evidence = read(&files, launched).unwrap_or_else(|| panic!("{status} is terminal"));
        assert_eq!(evidence["failure"]["status"], status);
    }
}

#[test]
fn must_fail_a_completion_artifact_for_another_cli_is_not_terminal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let files = files_in(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    let artifact = completion(&files, "failed", launched + 100);
    fs::write(&files.completion_status_path, artifact.to_string()).expect("write");
    let claude = read_agent_spawn_terminal_failure(
        &files,
        ActSpawnAgentCli::Claude,
        SPAWN_ID,
        launched,
        LAUNCHER_PID,
    );
    assert!(
        claude.is_none(),
        "artifact recorded for codex, read as claude"
    );
    let mut claude_artifact = completion(&files, "failed", launched + 100);
    claude_artifact["cli"] = json!("claude");
    fs::write(&files.completion_status_path, claude_artifact.to_string()).expect("write");
    let evidence = read_agent_spawn_terminal_failure(
        &files,
        ActSpawnAgentCli::Claude,
        SPAWN_ID,
        launched,
        LAUNCHER_PID,
    )
    .expect("claude completion failure is terminal");
    assert_eq!(evidence["source"], "completion_status");
}

fn files_with_codex_control(root: &std::path::Path) -> (AgentSpawnFiles, std::path::PathBuf) {
    let mut files = files_in(root);
    let control = files.log_dir.join("codex-app-server-control.json");
    files.codex_app_server_control_path = Some(control.clone());
    files.codex_app_server_events_path = Some(files.log_dir.join("codex-app-server-events.jsonl"));
    (files, control)
}

fn control_artifact(files: &AgentSpawnFiles, status: &str, updated_at: u64) -> Value {
    json!({
        "schema_version": 1,
        "protocol": "codex_app_server_ws",
        "control_path": files
            .codex_app_server_control_path
            .as_ref()
            .expect("control path")
            .display()
            .to_string(),
        "events_path": files
            .codex_app_server_events_path
            .as_ref()
            .expect("events path")
            .display()
            .to_string(),
        "updated_at_unix_ms": updated_at,
        "turn_status": status,
    })
}

#[test]
fn must_pass_codex_control_runner_error_and_failed_are_terminal_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (files, control) = files_with_codex_control(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    for status in ["runner_error", "failed"] {
        let artifact = control_artifact(&files, status, launched + 100);
        fs::write(&control, artifact.to_string()).expect("write control");
        let evidence = read(&files, launched).unwrap_or_else(|| panic!("{status} is terminal"));
        assert_eq!(evidence["reason"], "spawn_terminal_failure");
        assert_eq!(evidence["source"], "codex_control");
        assert_eq!(evidence["failure"]["turn_status"], status);
    }
}

#[test]
fn must_fail_codex_control_non_failure_stale_foreign_or_partial_is_not_terminal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (files, control) = files_with_codex_control(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    assert!(read(&files, launched).is_none(), "no control artifact");

    fs::write(&control, "{\"schema_version\":").expect("partial");
    assert!(read(&files, launched).is_none(), "partial control artifact");

    let running = control_artifact(&files, "running", launched + 100);
    fs::write(&control, running.to_string()).expect("running");
    assert!(read(&files, launched).is_none(), "non-failure turn status");

    let stale = control_artifact(&files, "runner_error", launched.saturating_sub(60_000));
    fs::write(&control, stale.to_string()).expect("stale");
    assert!(read(&files, launched).is_none(), "update older than launch");

    let mut foreign = control_artifact(&files, "runner_error", launched + 100);
    foreign["control_path"] = json!("C:/elsewhere/control.json");
    fs::write(&control, foreign.to_string()).expect("foreign");
    assert!(read(&files, launched).is_none(), "control path mismatch");

    let mut other_protocol = control_artifact(&files, "runner_error", launched + 100);
    other_protocol["protocol"] = json!("something_else");
    fs::write(&control, other_protocol.to_string()).expect("protocol");
    assert!(read(&files, launched).is_none(), "protocol mismatch");
}

#[test]
fn must_fail_codex_control_artifact_is_ignored_for_other_clis() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (files, control) = files_with_codex_control(temp.path());
    let launched = unix_time_ms_now().saturating_sub(5_000);
    let artifact = control_artifact(&files, "runner_error", launched + 100);
    fs::write(&control, artifact.to_string()).expect("write control");
    let claude = read_agent_spawn_terminal_failure(
        &files,
        ActSpawnAgentCli::Claude,
        SPAWN_ID,
        launched,
        LAUNCHER_PID,
    );
    assert!(claude.is_none());
}
