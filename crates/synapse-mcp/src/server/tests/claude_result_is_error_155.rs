//! Regression tests for #155: a Claude stream-json `result` event with
//! `is_error: true` is a failed spawn even when the CLI exits 0. The test runs
//! the real generated wrapper script against a fake `claude.cmd`.

#![cfg(windows)]

use super::*;

const SPAWN_ID: &str = "agent-spawn-result-is-error-155";

fn claude_files(dir: &Path) -> AgentSpawnFiles {
    AgentSpawnFiles {
        log_dir: dir.to_path_buf(),
        prompt_path: dir.join("prompt.txt"),
        stdout_path: dir.join("stdout.jsonl"),
        stderr_path: dir.join("stderr.log"),
        final_message_path: dir.join("final-message.txt"),
        completion_status_path: dir.join("completion-status.json"),
        task_started_path: dir.join("task-started.json"),
        task_started_script_path: dir.join("write-task-started.ps1"),
        debug_path: Some(dir.join("claude-debug.log")),
        mcp_config_path: Some(dir.join("claude-mcp-config.json")),
        hook_settings_path: Some(dir.join("claude-hook-settings.json")),
        notify_script_path: None,
        codex_app_server_runner_path: None,
        codex_app_server_control_path: None,
        codex_app_server_events_path: None,
        codex_app_server_stdout_path: None,
        codex_app_server_stderr_path: None,
        local_model_runner_path: None,
    }
}

/// Runs the wrapper with a fake `claude` that prints `stream` and exits 0, and
/// returns the completion-status artifact it wrote.
fn run_wrapper_with_fake_claude(stream: &str) -> Value {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin dir");
    fs::write(bin.join("fake-stream.jsonl"), stream).expect("fake stream");
    fs::write(
        bin.join("claude.cmd"),
        "@echo off\r\ntype \"%~dp0fake-stream.jsonl\"\r\nexit /b 0\r\n",
    )
    .expect("fake claude");
    fs::write(dir.join("prompt.txt"), "hello").expect("prompt");

    let files = claude_files(dir);
    let params = ActSpawnAgentParams {
        cli: Some(ActSpawnAgentCli::Claude),
        kind: None,
        model: None,
        model_ref: None,
        prompt: Some("hello".to_owned()),
        target: None,
        working_dir: None,
        mcp_url: "http://127.0.0.1:7700/mcp".to_owned(),
        wait_timeout_ms: 30_000,
        hold_open_ms: 0,
        require_approval_gate: true,
        template_id: None,
        template_version: None,
        template_config_hash: None,
    };
    let script = agent_spawn_powershell_script(&params, &files, dir).expect("wrapper script");
    let script_path = dir.join("wrapper.ps1");
    fs::write(&script_path, script).expect("write wrapper");

    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let powershell = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let mut path = std::ffi::OsString::from(&bin);
    path.push(";");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let output = std::process::Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .env("PATH", path)
        .env("SYNAPSE_AGENT_SPAWN_ID", SPAWN_ID)
        .output()
        .expect("run wrapper");
    let status_bytes = fs::read(dir.join("completion-status.json")).unwrap_or_else(|error| {
        panic!(
            "completion status missing ({error}); stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let status_bytes = status_bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(&status_bytes);
    serde_json::from_slice(status_bytes).expect("completion status JSON")
}

const ASSISTANT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}]}}"#;

#[test]
fn must_fail_result_event_with_is_error_true_and_exit_zero_is_failed() {
    let stream = format!(
        "{ASSISTANT}\n{}\n",
        r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: 529 overloaded"}"#
    );
    let status = run_wrapper_with_fake_claude(&stream);
    assert_eq!(status["status"], "failed", "{status}");
    assert_eq!(
        status["exit_code"], 0,
        "CLI exit code stays honest: {status}"
    );
    assert!(
        status["error_message"]
            .as_str()
            .is_some_and(|message| message.contains("is_error=true")),
        "{status}"
    );
    assert_eq!(status["final_message_present"], true, "{status}");
}

#[test]
fn must_pass_result_event_without_is_error_or_false_stays_ok() {
    for result in [
        r#"{"type":"result","subtype":"success","result":"done"}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#,
    ] {
        let status = run_wrapper_with_fake_claude(&format!("{ASSISTANT}\n{result}\n"));
        assert_eq!(status["status"], "ok", "{status}");
    }
}

#[test]
fn must_pass_the_last_result_event_decides_is_error() {
    let stream = format!(
        "{ASSISTANT}\n{}\n{}\n",
        r#"{"type":"result","is_error":true,"result":"first attempt failed"}"#,
        r#"{"type":"result","is_error":false,"result":"retry succeeded"}"#
    );
    let status = run_wrapper_with_fake_claude(&stream);
    assert_eq!(status["status"], "ok", "{status}");
}
