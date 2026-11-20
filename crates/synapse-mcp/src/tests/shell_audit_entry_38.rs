//! #38: the command line the real authorization gate matched and the durable audit row
//! hashed must be the bytes the child process received from `CreateProcessW`.
//!
//! `shell_audit_command_line_38` compares the serializer with a child that the test
//! spawns itself; this file goes through `authorize_run_shell_start` and
//! `start_authorized_shell_job` and reads the persisted `status.json` back.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use super::*;

const DEADLINE: Duration = Duration::from_secs(30);
const JOB_ID: &str = "issue38-audit-entry";

fn config(pattern: &str) -> M4ServiceConfig {
    M4ServiceConfig::from_parts_with_any_env(
        vec![pattern.to_owned()],
        Vec::new(),
        DEFAULT_RUN_SHELL_INLINE_AWAIT_LIMIT_MS,
        None,
        None,
    )
    .unwrap_or_else(|error| panic!("config: {error:#}"))
}

fn start_params(script: &Path) -> ActRunShellStartParams {
    let script_text = script.to_string_lossy();
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script_text.as_ref(),
        "a\"b",
        "two words",
        "my dir\\",
        "back\\\\\"slash",
        "",
    ];
    ActRunShellStartParams {
        command: "powershell.exe".to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        working_dir: None,
        env: BTreeMap::new(),
        timeout_ms: Some(20_000),
        job_id: Some(JOB_ID.to_owned()),
    }
}

fn write_script(dir: &Path) -> PathBuf {
    let script = dir.join("cmdline38.ps1");
    std::fs::write(
        &script,
        "[Console]::Out.Write([Environment]::CommandLine)\n",
    )
    .unwrap_or_else(|error| panic!("write script: {error}"));
    script
}

async fn wait_terminal(paths: &ShellJobPaths) -> ActRunShellJobStatus {
    let started = Instant::now();
    loop {
        let job = read_shell_job_status(&paths.status_path, JOB_ID)
            .unwrap_or_else(|error| panic!("status: {error:?}"));
        if shell_job_terminal_status(&job.status) {
            return job;
        }
        assert!(started.elapsed() < DEADLINE, "job never finished: {job:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_authorized_and_audited_line_is_the_child_command_line_38() {
    let _root = ShellJobRootGuard::new();
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let params = start_params(&write_script(dir.path()));
    let command_line = shell_command_line_from_parts(&params.command, &params.args);
    let pattern = format!("^{}$", regex::escape(&command_line));
    let authorization = authorize_run_shell_start(&config(&pattern), &params)
        .unwrap_or_else(|error| panic!("authorize: {error:?}"));

    start_authorized_shell_job(params, &authorization, None)
        .unwrap_or_else(|error| panic!("start: {error:?}"));
    let paths =
        shell_job_paths_for_id(None, JOB_ID).unwrap_or_else(|error| panic!("paths: {error:?}"));
    let job = wait_terminal(&paths).await;
    let received = std::fs::read_to_string(&paths.stdout_path)
        .unwrap_or_else(|error| panic!("stdout.log: {error}"));

    assert_eq!(job.status, "ok", "{job:?}");
    assert_eq!(
        authorization.command_line, received,
        "policy matched other bytes"
    );
    assert_eq!(job.command_line_original_bytes, received.len());
    assert_eq!(
        job.command_line_sha256,
        crate::content_digest::keyed_prefixed_digest(received.as_bytes()),
        "audit hash must name the OS command line"
    );
}

#[test]
fn must_fail_pattern_written_for_a_cosmetic_wrapped_rendering_is_refused_38() {
    let _root = ShellJobRootGuard::new();
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let params = start_params(&write_script(dir.path()));
    // Matches only the old rendering that wrapped the quote-only argument: "a\"b".
    let actual = shell_command_line_from_parts(&params.command, &params.args);
    let wrapped = actual.replace(r#" a\"b "#, r#" "a\"b" "#);
    assert_ne!(
        wrapped, actual,
        "fixture must change the quote-only argument"
    );
    let cosmetic = config(&format!("^{}$", regex::escape(&wrapped)));

    let error = authorize_run_shell_start(&cosmetic, &params)
        .err()
        .unwrap_or_else(|| panic!("cosmetic rendering must not authorize"));

    let code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str);
    assert_eq!(code, Some(error_codes::SAFETY_SHELL_DENIED_BY_POLICY));
    let paths =
        shell_job_paths_for_id(None, JOB_ID).unwrap_or_else(|error| panic!("paths: {error:?}"));
    assert!(
        !paths.job_dir.exists(),
        "a refused start must not create a job dir"
    );
}
