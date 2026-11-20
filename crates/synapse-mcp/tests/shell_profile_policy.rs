//! #21 shell profile policy driven through the real `shell` facade over the
//! HTTP daemon (per-session tool profiles), not the policy helper.
//!
//! Fixture: an isolated daemon with NO operator allowlist. By design local
//! access is open, so the operator gate reports `allow_shell_patterns=any`
//! and admits every command (contract: src/m4.rs `from_parts_with_any_env`
//! "any is the permissive default ... for a family with no configured
//! patterns"; rendered by src/server/health.rs `action_health`). Each test
//! reads that precondition back. The only gate under test is therefore the
//! per-session PROFILE gate (src/server/shell_profile_policy.rs
//! `allows_shell_command`, called from src/server/m4_tools.rs
//! `require_shell_command_profile_policy`).

#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};
use synapse_core::error_codes;
use synapse_storage::{Db, cf, decode_json};

const CAPABILITY: &str = "syn21-isolated-local-agent-capability-0123456789";
const WHOAMI: &str = r"C:\Windows\System32\whoami.exe";
const HOSTNAME: &str = r"C:\Windows\System32\hostname.exe";

async fn start_daemon() -> anyhow::Result<HttpDaemon> {
    // Synthetic notepad foreground: the action scope gate that precedes the
    // shell profile gate must not depend on the host's real foreground.
    HttpDaemon::start(&[
        ("SYNAPSE_LOCAL_AGENT_CAPABILITY", CAPABILITY),
        ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
    ])
    .await
}

/// Full-capability session: the trusted local-agent client name plus the
/// capability header (contract: tests/local_agent_profile_trust.rs
/// must_pass_local_agent_name_with_the_capability_gets_full_capability).
async fn full_capability_session(daemon: &HttpDaemon) -> anyhow::Result<HttpMcpSession> {
    daemon
        .session(
            "synapse-local-model-agent",
            &[("X-Synapse-Local-Agent-Capability", CAPABILITY)],
        )
        .await
}

async fn effective_profile(session: &mut HttpMcpSession) -> anyhow::Result<Value> {
    let status = session
        .tools_call("profile", json!({"operation": "status"}))
        .await?;
    Ok(status["structuredContent"]["status"]["snapshot"]["profile"].clone())
}

/// Fixture precondition readback: no operator allowlist is configured, so
/// the operator gate is open (`any`) for both families (health.rs
/// `action_health`).
async fn assert_operator_gate_open(session: &mut HttpMcpSession) -> anyhow::Result<()> {
    let health = session.tools_call("health", json!({})).await?;
    let detail = health["structuredContent"]["subsystems"]["action"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        detail.contains("allow_shell_patterns=any allow_launch_patterns=any"),
        "fixture precondition: isolated daemon must have the open operator gate: {detail}"
    );
    Ok(())
}

fn shell_args(command: &str, args: &[&str], extra: &Value) -> Value {
    let mut call = json!({"operation": "run", "command": command, "args": args});
    if let (Some(call), Some(extra)) = (call.as_object_mut(), extra.as_object()) {
        call.extend(extra.clone());
    }
    call
}

async fn shell_refused(
    session: &mut HttpMcpSession,
    command: &str,
    args: &[&str],
    extra: Value,
) -> anyhow::Result<Value> {
    session
        .tools_call_error("shell", shell_args(command, args, &extra))
        .await
}

/// A successful inline run; returns the `run` readback
/// (src/m4.rs `ActRunShellResponse`).
async fn shell_ok(
    session: &mut HttpMcpSession,
    command: &str,
    args: &[&str],
) -> anyhow::Result<Value> {
    let extra = json!({"execution_mode": "inline", "timeout_ms": 30_000});
    let response = session
        .tools_call("shell", shell_args(command, args, &extra))
        .await?;
    let run = response["structuredContent"]["run"].clone();
    assert_eq!(run["exit_code"], 0, "{command}: {run}");
    assert_eq!(run["timed_out"], false, "{command}: {run}");
    // m4.rs RunShellOutput: `backgrounded` is omitted when false (skip_serializing_if = "is_false").
    assert!(
        run.get("backgrounded")
            .is_none_or(|value| value == &Value::Bool(false)),
        "{command}: {run}"
    );
    assert!(run["error_code"].is_null(), "{command}: {run}");
    Ok(run)
}

/// The host's own output for the same harmless command, run independently of
/// the daemon, as the expected stdout.
fn local_stdout(command: &str) -> anyhow::Result<String> {
    let output = std::process::Command::new(command).output()?;
    anyhow::ensure!(output.status.success(), "local {command} failed");
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Profile-gate refusal (m4_tools.rs `require_shell_command_profile_policy`
/// message: "shell command is not permitted for profile {profile}").
fn assert_profile_denied(error: &Value, profile: &str) {
    assert_eq!(
        error["data"]["code"],
        error_codes::TOOL_PROFILE_POLICY_DENIED,
        "{error}"
    );
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&format!("not permitted for profile {profile}")),
        "denial must come from the shell profile gate: {error}"
    );
}

/// No shell child ever launched: the daemon recorded no process history.
fn assert_no_process_history(db_path: &std::path::Path) -> anyhow::Result<()> {
    let db = Db::open(db_path, synapse_core::SCHEMA_VERSION)?;
    let rows = db.scan_cf_prefix(cf::CF_PROCESS_HISTORY, b"")?;
    assert_eq!(rows.len(), 0, "no process may have been launched");
    Ok(())
}

/// Execution audit readback: the `final` command-audit rows the daemon wrote
/// to CF_ACTION_LOG for `act_run_shell` in `session_id` (contract:
/// src/server/command_audit.rs row shape; m4_tools.rs act_run_shell writes
/// outcome "ok" with `after.response` on success).
fn shell_final_audits(db_path: &std::path::Path, session_id: &str) -> anyhow::Result<Vec<Value>> {
    let db = Db::open(db_path, synapse_core::SCHEMA_VERSION)?;
    let mut rows = Vec::new();
    for (_key, value) in db.scan_cf(cf::CF_ACTION_LOG)? {
        let Ok(row) = decode_json::<Value>(&value) else {
            continue;
        };
        if row["row_kind"] == "command_audit"
            && row["phase"] == "final"
            && row["tool"] == "act_run_shell"
            && row["actor"]["session_id"] == session_id
        {
            rows.push(row);
        }
    }
    Ok(rows)
}

#[tokio::test]
async fn must_fail_normal_agent_shell_outside_allowlist_is_refused_by_profile() -> anyhow::Result<()>
{
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("syn21-normal-agent", &[]).await?;
    assert_eq!(effective_profile(&mut session).await?, "normal_agent");
    assert_operator_gate_open(&mut session).await?;

    // normal_agent admits only the protected allowlist with no args, env or
    // cwd (shell_profile_policy.rs NORMAL_AGENT_SHELL_ALLOWLIST +
    // allows_shell_command): unlisted interpreter, allowlisted exe with args,
    // with env, with cwd are all refused.
    let cmd = shell_refused(
        &mut session,
        "cmd.exe",
        &["/c", "echo unapproved"],
        json!({}),
    )
    .await?;
    assert_profile_denied(&cmd, "normal_agent");
    let with_args = shell_refused(&mut session, WHOAMI, &["/all"], json!({})).await?;
    assert_profile_denied(&with_args, "normal_agent");
    let with_env = shell_refused(&mut session, WHOAMI, &[], json!({"env": {"SYN21": "1"}})).await?;
    assert_profile_denied(&with_env, "normal_agent");
    let cwd = daemon.home().to_string_lossy().into_owned();
    let with_cwd = shell_refused(&mut session, WHOAMI, &[], json!({"working_dir": cwd})).await?;
    assert_profile_denied(&with_cwd, "normal_agent");

    let _home = daemon.stop().await?;
    assert_no_process_history(&db_path)
}

#[tokio::test]
async fn must_pass_normal_agent_allowlisted_command_passes_profile_gate() -> anyhow::Result<()> {
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("syn21-normal-agent", &[]).await?;
    let session_id = session.session_id().to_owned();
    assert_eq!(effective_profile(&mut session).await?, "normal_agent");
    assert_operator_gate_open(&mut session).await?;

    // whoami/hostname with no args are explicitly admitted for normal_agent
    // (shell_profile_policy.rs NORMAL_AGENT_SHELL_ALLOWLIST, matched
    // case-insensitively by allows_shell_command). Expected output is the
    // host's own independent run of the same command.
    let cases = [
        (WHOAMI, local_stdout(WHOAMI)?),
        (HOSTNAME, local_stdout(HOSTNAME)?),
        (r"c:\windows\system32\WHOAMI.EXE", local_stdout(WHOAMI)?),
    ];
    for (command, expected) in &cases {
        let run = shell_ok(&mut session, command, &[]).await?;
        assert_eq!(run["stdout"], expected.as_str(), "{command}: {run}");
    }

    let _home = daemon.stop().await?;
    let audits = shell_final_audits(&db_path, &session_id)?;
    assert_eq!(
        audits.len(),
        cases.len(),
        "one final execution audit per admitted run: {audits:?}"
    );
    for audit in &audits {
        assert_eq!(audit["outcome"], "ok", "{audit}");
        assert_eq!(audit["after"]["response"]["exit_code"], 0, "{audit}");
    }
    Ok(())
}

#[tokio::test]
async fn must_pass_full_capability_shell_passes_profile_gate() -> anyhow::Result<()> {
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = full_capability_session(&daemon).await?;
    let session_id = session.session_id().to_owned();
    assert_eq!(effective_profile(&mut session).await?, "full_capability");
    assert_operator_gate_open(&mut session).await?;

    // full_capability admits any shell command (shell_profile_policy.rs
    // allows_shell_command: "break_glass" | "full_capability" => true).
    let run = shell_ok(&mut session, "cmd.exe", &["/c", "echo approved"]).await?;
    assert_eq!(run["stdout"], "approved\r\n", "{run}");

    let _home = daemon.stop().await?;
    let audits = shell_final_audits(&db_path, &session_id)?;
    assert_eq!(audits.len(), 1, "one final execution audit: {audits:?}");
    assert_eq!(audits[0]["outcome"], "ok", "{}", audits[0]);
    assert_eq!(
        audits[0]["after"]["response"]["stdout"], "approved\r\n",
        "{}",
        audits[0]
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_browser_debugger_profile_cannot_run_shell() -> anyhow::Result<()> {
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("syn21-browser-debugger", &[]).await?;
    session
        .tools_call(
            "profile",
            json!({
                "operation": "set",
                "profile": "browser_debugger",
                "confirm_break_glass": true,
                "reason": "syn21 test: browser_debugger must not gain shell"
            }),
        )
        .await?;
    assert_eq!(effective_profile(&mut session).await?, "browser_debugger");
    assert_operator_gate_open(&mut session).await?;

    // browser_debugger falls to the `_ => false` arm of allows_shell_command:
    // even the normal_agent allowlist is refused.
    for (command, args) in [("cmd.exe", &[][..]), (WHOAMI, &[][..])] {
        let error = shell_refused(&mut session, command, args, json!({})).await?;
        assert_profile_denied(&error, "browser_debugger");
    }

    let _home = daemon.stop().await?;
    assert_no_process_history(&db_path)
}
