//! #155: the real generated spawn wrapper must recover the Claude CLI's FINAL answer as
//! `final-message.txt` — exact bytes — not just report `status=ok`.
//!
//! `claude_result_is_error_155` asserts the status only, so a wrapper that kept an
//! earlier assistant message ("working") instead of the final result ("done") would
//! still pass. Each test runs the wrapper against a fake `claude.cmd` that prints a
//! scripted stream-json and exits 0, then reads both artifacts back.

#![cfg(windows)]

use super::*;

const SPAWN_ID: &str = "agent-spawn-final-message-155";
const WORKING: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}]}}"#;
const ALMOST: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"almost"}]}}"#;

fn files(dir: &Path) -> AgentSpawnFiles {
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

fn params() -> ActSpawnAgentParams {
    ActSpawnAgentParams {
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
    }
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

struct Outcome {
    status: Value,
    final_message: Option<String>,
}

fn run(stream: &str) -> Outcome {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let dir = temp.path();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("bin: {error}"));
    fs::write(bin.join("fake-stream.jsonl"), stream).unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        bin.join("claude.cmd"),
        "@echo off\r\ntype \"%~dp0fake-stream.jsonl\"\r\nexit /b 0\r\n",
    )
    .unwrap_or_else(|error| panic!("fake claude: {error}"));
    fs::write(dir.join("prompt.txt"), "hello").unwrap_or_else(|e| panic!("{e}"));
    let script = agent_spawn_powershell_script(&params(), &files(dir), dir)
        .unwrap_or_else(|error| panic!("wrapper script: {error:?}"));
    let script_path = dir.join("wrapper.ps1");
    fs::write(&script_path, script).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !dir.join("final-message.txt").exists(),
        "precondition: no final message"
    );

    let mut path = std::ffi::OsString::from(&bin);
    path.push(";");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let powershell =
        PathBuf::from(system_root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    std::process::Command::new(powershell)
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
        .unwrap_or_else(|error| panic!("run wrapper: {error}"));

    let status_bytes = fs::read(dir.join("completion-status.json"))
        .unwrap_or_else(|error| panic!("completion status: {error}"));
    let status = serde_json::from_slice(strip_bom(&status_bytes))
        .unwrap_or_else(|error| panic!("status JSON: {error}"));
    let final_message = fs::read(dir.join("final-message.txt"))
        .ok()
        .map(|bytes| String::from_utf8_lossy(strip_bom(&bytes)).into_owned());
    Outcome {
        status,
        final_message,
    }
}

#[test]
fn must_pass_final_message_is_the_result_text_not_an_earlier_assistant_message_155() {
    let result = r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#;
    let outcome = run(&format!("{WORKING}\n{result}\n"));

    assert_eq!(outcome.status["status"], "ok", "{}", outcome.status);
    assert_eq!(outcome.final_message.as_deref(), Some("done\r\n"));
    assert_eq!(
        outcome.status["final_message_source"], "stdout_jsonl_result",
        "{}",
        outcome.status
    );
    assert_eq!(
        outcome.status["recovered_final_message_written"], true,
        "{}",
        outcome.status
    );
}

#[test]
fn must_pass_assistant_only_stream_keeps_the_last_assistant_text_155() {
    let outcome = run(&format!("{WORKING}\n{ALMOST}\n"));

    assert_eq!(outcome.status["status"], "ok", "{}", outcome.status);
    assert_eq!(outcome.final_message.as_deref(), Some("almost\r\n"));
    assert_eq!(
        outcome.status["final_message_source"], "stdout_jsonl_claude_assistant_message",
        "{}",
        outcome.status
    );
}

#[test]
fn must_fail_is_error_result_is_failed_and_keeps_the_error_text_155() {
    let result = r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: 529 overloaded"}"#;
    let outcome = run(&format!("{WORKING}\n{result}\n"));

    assert_eq!(outcome.status["status"], "failed", "{}", outcome.status);
    assert_eq!(
        outcome.final_message.as_deref(),
        Some("API Error: 529 overloaded\r\n")
    );
}

#[test]
fn must_fail_stream_without_any_answer_is_missing_final_response_155() {
    let outcome = run("{\"type\":\"system\",\"subtype\":\"init\"}\nnot json at all\n");

    assert_eq!(
        outcome.status["status"], "missing_final_response",
        "{}",
        outcome.status
    );
    assert_eq!(
        outcome.status["recovered_final_message_written"], false,
        "{}",
        outcome.status
    );
    let fallback = outcome.final_message.unwrap_or_default();
    assert!(
        !fallback.contains("working"),
        "no invented answer: {fallback}"
    );
}
