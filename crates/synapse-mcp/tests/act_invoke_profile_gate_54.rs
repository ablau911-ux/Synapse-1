//! #54: `act invoke` must not let a normal_agent session reach a hidden
//! foreground-tier / shell delegate that the MCP dispatch boundary denies when
//! called directly. `run_shell` re-checks `act_run_shell` admission and
//! `focus_window` re-checks the foreground tier; both refuse before delivery.
//!
//! Contract: issue #54 comments (codex 2026-09-14: "normal-agent act.invoke
//! run_shell whoami ... returns TOOL_PROFILE_POLICY_DENIED; shell.run exact
//! whoami still exits 0"; manager VBV 2026-10-14: invoke type / focus_window
//! refused with TOOL_PROFILE_POLICY_DENIED before delivery);
//! server/background_router.rs `target_act` verb=run_shell
//! (`admit_tool_call_for_profile("act_run_shell")`).
//!
//! Source of truth for "nothing ran": CF_PROCESS_HISTORY in the daemon's DB
//! (same readback as tests/shell_profile_policy.rs). `verb=type` is NOT driven
//! here: if its gate regressed it would send real keystrokes to the owner's
//! foreground window; it stays a manual VBV item (criteria_054.md).

#![cfg(windows)]

mod g4_support;
#[allow(dead_code)]
mod support;

use g4_support::{act_invoke_refused, nested_code, normal_session, synthetic_daemon};
use serde_json::json;
use synapse_core::error_codes;
use synapse_storage::{Db, cf};

const WHOAMI: &str = r"C:\Windows\System32\whoami.exe";

fn process_history_rows(db_path: &std::path::Path) -> anyhow::Result<usize> {
    let db = Db::open(db_path, synapse_core::SCHEMA_VERSION)?;
    Ok(db.scan_cf_prefix(cf::CF_PROCESS_HISTORY, b"")?.len())
}

#[tokio::test]
async fn must_fail_normal_agent_run_shell_via_act_invoke_is_denied_and_runs_nothing()
-> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = normal_session(&daemon, "g4-54-run-shell").await?;

    // whoami with no args is on the normal_agent SHELL allowlist; the hidden
    // act_run_shell delegate must still be refused on the act route. The
    // admission check returns Err from target_act (background_router.rs
    // `"run_shell"` arm: `admit_tool_call_for_profile(...)?`), so the whole
    // call is a JSON-RPC error.
    let error = session
        .tools_call_error(
            "act",
            json!({"operation": "invoke", "action": {"verb": "run_shell", "command": WHOAMI}}),
        )
        .await?;

    assert_eq!(
        error["data"]["code"],
        error_codes::TOOL_PROFILE_POLICY_DENIED,
        "{error}"
    );
    let _home = daemon.stop().await?;
    assert_eq!(
        process_history_rows(&db_path)?,
        0,
        "no process may have been launched"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_normal_agent_focus_window_via_act_invoke_is_denied() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-54-focus").await?;

    let structured = act_invoke_refused(&mut session, json!({"verb": "focus_window"})).await?;

    assert_eq!(
        structured["action"]["delegated_tool"], "act_focus_window",
        "{structured}"
    );
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED),
        "{structured}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: the public `shell` facade still runs the allowlisted command.
#[tokio::test]
async fn must_pass_normal_agent_shell_facade_still_runs_allowlisted_whoami() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-54-shell-facade").await?;
    let expected = std::process::Command::new(WHOAMI).output()?;
    assert!(expected.status.success(), "precondition: local whoami runs");

    let response = session
        .tools_call(
            "shell",
            json!({"operation": "run", "command": WHOAMI, "args": [],
                   "execution_mode": "inline", "timeout_ms": 30_000}),
        )
        .await?;

    let run = &response["structuredContent"]["run"];
    assert_eq!(run["exit_code"], 0, "{run}");
    assert_eq!(
        run["stdout"],
        String::from_utf8_lossy(&expected.stdout).as_ref(),
        "{run}"
    );
    drop(daemon.stop().await?);
    Ok(())
}
