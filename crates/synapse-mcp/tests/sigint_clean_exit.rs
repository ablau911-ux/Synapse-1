#![cfg(windows)]
//! Console-signal shutdown of the stdio daemon on Windows.
//!
//! The daemon's real shutdown path (`wait_for_shutdown_signal` in main.rs)
//! listens for Ctrl+C and Ctrl+Break. The child is launched as the leader of a
//! new console process group, so `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT,
//! pid)` reaches only the daemon, never the test process. Stdin stays open the
//! whole time, so the exit cannot come from the stdin-EOF path.

use std::time::Duration;

use anyhow::Context;
use serde_json::{Value, json};
use synapse_core::error_codes;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};

#[tokio::test]
async fn must_pass_ctrl_break_results_in_exit_0_and_flushed_log() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let client = StdioMcpClient::launch_and_init_in_new_process_group(Some(dir.path())).await?;
    let before_logs = read_logs(dir.path())?;
    let before_shutdown = event_code_count(&before_logs, "MCP_SHUTDOWN_GRACEFUL");
    let before_safety = safety_reason_lines(&before_logs, "sigint").len();
    println!(
        "readback=daemon_log edge=ctrl_break before_shutdown_count={before_shutdown} before_safety_count={before_safety}"
    );
    assert_eq!(
        before_shutdown, 0,
        "no shutdown before the signal: {before_logs}"
    );
    assert_eq!(
        before_safety, 0,
        "no release_all before the signal: {before_logs}"
    );

    let pid = client.child_id().context("daemon pid missing")?;
    // SAFETY: plain Win32 call with two integer arguments; `pid` is the
    // process-group id of the child spawned with CREATE_NEW_PROCESS_GROUP.
    unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) }.context(
        "GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT) failed; the test process needs a console",
    )?;
    let status = client.wait_for_exit(Duration::from_secs(10)).await?;

    let logs = read_logs(dir.path())?;
    let shutdown_count = event_code_count(&logs, "MCP_SHUTDOWN_GRACEFUL");
    let safety_lines = safety_reason_lines(&logs, "sigint");
    println!(
        "readback=daemon_log edge=ctrl_break after_shutdown_count={shutdown_count} after_safety_count={} after_line={:?} exit_code={:?}",
        safety_lines.len(),
        safety_lines.first(),
        status.code()
    );
    assert_eq!(status.code(), Some(0), "logs: {logs}");
    assert_eq!(shutdown_count, 1, "exactly one graceful shutdown: {logs}");
    assert!(
        !safety_lines.is_empty(),
        "expected shutdown release_all safety log with reason sigint, got logs: {logs}"
    );
    assert_eq!(
        event_code_count(&logs, "MCP_STDIO_EOF_CONNECTION_CLOSED"),
        0,
        "exit must come from the console signal, not stdin EOF: {logs}"
    );
    assert!(
        safety_reason_lines(&logs, "connection_closed").is_empty(),
        "signal shutdown must not be reported as connection_closed: {logs}"
    );
    Ok(())
}

#[tokio::test]
async fn must_pass_stdio_connection_closed_emits_release_all_log() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let mut client = StdioMcpClient::launch_and_init_with_log_dir(Some(dir.path())).await?;
    // The raw pad primitive is refused on the unscoped stdio lane (#58), so no
    // pad can be held when the connection closes.
    let pad_error = client
        .tools_call_error(
            "act_pad",
            json!({
                "pad_id": 4,
                "report": {
                    "buttons": ["a"],
                    "rt": 0.5
                }
            }),
        )
        .await?;
    assert_eq!(
        pad_error["data"]["code"],
        error_codes::TOOL_PROFILE_POLICY_DENIED,
        "{pad_error}"
    );
    let before_logs = read_logs(dir.path())?;
    assert!(
        safety_reason_lines(&before_logs, "connection_closed").is_empty(),
        "no connection_closed release before stdin closes: {before_logs}"
    );

    let status = client.shutdown().await?;
    assert_eq!(status.code(), Some(0));

    let logs = read_logs(dir.path())?;
    let safety_lines = safety_reason_lines(&logs, "connection_closed");
    let safety_line = safety_lines
        .first()
        .context("connection_closed safety line missing")?;
    println!(
        "readback=daemon_log edge=connection_closed after_safety_count={} after_line={safety_line:?}",
        safety_lines.len()
    );
    assert!(
        safety_line.contains("\"held_pad_ids\":\"[]\""),
        "{safety_line}"
    );
    assert!(safety_line.contains("\"released_pads\":0"), "{safety_line}");
    assert!(
        safety_reason_lines(&logs, "sigint").is_empty(),
        "stdin EOF must not be reported as a signal shutdown: {logs}"
    );
    Ok(())
}

fn read_logs(path: &std::path::Path) -> anyhow::Result<String> {
    let mut logs = String::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            logs.push_str(&std::fs::read_to_string(entry.path())?);
        }
    }
    Ok(logs)
}

fn safety_reason_lines(logs: &str, reason: &str) -> Vec<String> {
    logs.lines()
        .filter(|line| {
            parse_log_fields(line).is_some_and(|fields| {
                fields.get("code").and_then(Value::as_str)
                    == Some(error_codes::SAFETY_RELEASE_ALL_FIRED)
                    && fields.get("reason").and_then(Value::as_str) == Some(reason)
            })
        })
        .map(ToOwned::to_owned)
        .collect()
}

fn event_code_count(logs: &str, code: &str) -> usize {
    logs.lines()
        .filter_map(parse_log_fields)
        .filter(|fields| fields.get("code").and_then(Value::as_str) == Some(code))
        .count()
}

fn parse_log_fields(line: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(line).ok()?;
    Some(value.get("fields")?.clone())
}
