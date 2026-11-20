//! #39: secrets a shell command prints must not reach the public response, the durable
//! status tails, or the on-disk job artifacts — proven through the real shell entry
//! points, not only the sanitizer helpers.
//!
//! The secret values are assembled inside PowerShell from fragments, so the literal
//! value never appears in the request args or the recorded command line: any hit in a
//! readback can only come from the child's output.

#![cfg(windows)]

use std::time::{Duration, Instant};

use super::*;

const MASK: &str = "[redacted-shell-output]";
const STDOUT_SECRET: &str = "rv39-durable-secret-val";
const STDERR_SECRET: &str = "rv39-err-token-val";
const BENIGN: &str = "normal diagnostic token_count=42";
const SCRIPT: &str = "Write-Output ('L01 SECRET_KEY=' + 'rv39-dur' + 'able-secret-val'); \
     [Console]::Error.WriteLine('export API_TOKEN=' + 'rv39-err' + '-token-val'); \
     Write-Output ('normal diagnostic ' + 'token_count=42')";
const DEADLINE: Duration = Duration::from_secs(30);

fn args() -> Vec<String> {
    ["-NoProfile", "-NonInteractive", "-Command", SCRIPT]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect()
}

fn assert_no_secret(surface: &str, text: &str) {
    assert!(
        !text.contains(STDOUT_SECRET),
        "{surface} leaked stdout secret: {text}"
    );
    assert!(
        !text.contains(STDERR_SECRET),
        "{surface} leaked stderr secret: {text}"
    );
}

#[test]
fn must_pass_fixture_never_carries_the_secret_in_its_request_39() {
    let joined = args().join(" ");
    assert_no_secret("request args", &joined);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_inline_response_carries_no_secret_and_keeps_diagnostics_39() {
    let params: ActRunShellParams = serde_json::from_value(json!({
        "command": "powershell.exe",
        "args": args(),
        "timeout_ms": 20_000,
        "execution_mode": "inline",
    }))
    .unwrap_or_else(|error| panic!("params: {error}"));
    let auth = RunShellAuthorization {
        command_line: shell_command_line(&params),
        matched_pattern: "issue39-synthetic".to_owned(),
    };

    let response = run_authorized_shell(params, &auth, 60_000, None)
        .await
        .unwrap_or_else(|error| panic!("inline run: {error:?}"));
    let serialized =
        serde_json::to_string(&response).unwrap_or_else(|error| panic!("serialize: {error}"));

    assert_eq!(response.exit_code, Some(0), "{response:?}");
    assert_no_secret("inline response", &serialized);
    assert!(response.stdout.contains(MASK), "{}", response.stdout);
    assert!(response.stderr.contains(MASK), "{}", response.stderr);
    assert!(
        response.stdout.contains(BENIGN),
        "diagnostics must survive: {}",
        response.stdout
    );
}

async fn run_durable(job_id: &str) -> ShellJobPaths {
    let params = ActRunShellStartParams {
        command: "powershell.exe".to_owned(),
        args: args(),
        working_dir: None,
        env: BTreeMap::new(),
        timeout_ms: Some(20_000),
        job_id: Some(job_id.to_owned()),
    };
    let auth = RunShellAuthorization {
        command_line: shell_command_line_from_parts(&params.command, &params.args),
        matched_pattern: "issue39-synthetic".to_owned(),
    };
    start_authorized_shell_job(params, &auth, None)
        .unwrap_or_else(|error| panic!("start: {error:?}"));
    let paths =
        shell_job_paths_for_id(None, job_id).unwrap_or_else(|error| panic!("paths: {error:?}"));
    let started = Instant::now();
    loop {
        let job = read_shell_job_status(&paths.status_path, job_id)
            .unwrap_or_else(|error| panic!("status: {error:?}"));
        if shell_job_terminal_status(&job.status) {
            assert_eq!(job.status, "ok", "{job:?}");
            return paths;
        }
        assert!(started.elapsed() < DEADLINE, "job never finished: {job:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn read_lossy(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("read {path:?}: {error}"));
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_durable_artifacts_and_status_carry_no_secret_39() {
    let _root = ShellJobRootGuard::new();
    let paths = run_durable("issue39-durable-entry").await;

    let stdout_log = read_lossy(&paths.stdout_path);
    assert_no_secret("stdout.log", &stdout_log);
    assert_no_secret("stderr.log", &read_lossy(&paths.stderr_path));
    assert_no_secret("status.json", &read_lossy(&paths.status_path));
    assert!(
        stdout_log.contains(BENIGN),
        "artifact diagnostics must survive: {stdout_log}"
    );

    let status = shell_job_status(
        &ActRunShellStatusParams {
            job_id: "issue39-durable-entry".to_owned(),
            tail_bytes: 4096,
        },
        None,
    )
    .unwrap_or_else(|error| panic!("status: {error:?}"));
    let serialized =
        serde_json::to_string(&status).unwrap_or_else(|error| panic!("serialize: {error}"));
    assert_no_secret("act_run_shell_status", &serialized);
    assert!(status.stdout_tail.contains(MASK), "{}", status.stdout_tail);
    assert!(
        status.stdout_tail.contains(BENIGN),
        "{}",
        status.stdout_tail
    );
}
