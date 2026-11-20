//! #170 through the REAL daemon binary: a second launch on a DB that a live
//! daemon owns must be refused by the single-instance lock (exit 3) BEFORE it
//! runs crash recovery, so the live daemon's recovery ledger
//! (`<db>/action_recovery.jsonl`) and held inputs stay untouched.
//!
//! The ledger is poisoned with an undecodable line: a launch that runs the
//! recovery pass fails loudly ("recover stale action inputs from previous
//! daemon", exit 1) instead of releasing real keys, so "recovery ran" is
//! observable without SendInput. Covers both `run_http` and `run_stdio`
//! (shared `main.rs::acquire_lock_then_recover`).

#[allow(dead_code)]
mod support;

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::Context;
use support::HttpDaemon;
use tokio::process::Command;

/// Mirrors `support::BEARER` (private there).
const BEARER: &str = "synapse-tests-http-bearer";
const POISON: &str = "not-a-recovery-event\n";
const RECOVERY_FAILURE: &str = "recover stale action inputs from previous daemon";

fn daemon_command(home: &Path, db: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"));
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("SYNAPSE_")
        {
            command.env_remove(&key);
        }
    }
    command
        .args(args)
        .arg("--db")
        .arg(db)
        .env("APPDATA", home)
        .env("LOCALAPPDATA", home)
        .env("SYNAPSE_LOG_DIR", home.join("logs-170"))
        .env("SYNAPSE_BEARER_TOKEN", BEARER)
        .env("SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn run_to_exit(mut command: Command) -> anyhow::Result<(Option<i32>, String)> {
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .context("launch did not exit within 30s")??;
    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

fn free_bind() -> anyhow::Result<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.to_string())
}

async fn assert_second_launch_leaves_live_daemon_untouched(args: &[&str]) -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let db = daemon.db_path().to_path_buf();
    let ledger = db.join("action_recovery.jsonl");
    let pid_file = db.join("daemon.pid");
    std::fs::write(&ledger, POISON)?;
    let ledger_before = std::fs::read(&ledger)?;
    let holder_before = std::fs::read_to_string(&pid_file).context("precondition: live lock")?;
    assert_eq!(
        ledger_before,
        POISON.as_bytes(),
        "precondition: poisoned ledger"
    );

    let (code, stderr) = run_to_exit(daemon_command(daemon.home(), &db, args)).await?;

    assert_eq!(
        code,
        Some(3),
        "second launch must be refused by the lock; stderr={stderr}"
    );
    assert!(
        !stderr.contains(RECOVERY_FAILURE),
        "second launch ran recovery: {stderr}"
    );
    assert_eq!(
        std::fs::read(&ledger)?,
        ledger_before,
        "live daemon ledger changed"
    );
    assert_eq!(
        std::fs::read_to_string(&pid_file)?,
        holder_before,
        "lock holder changed"
    );
    let health = reqwest::Client::new()
        .get(daemon.url("/health"))
        .bearer_auth(BEARER)
        .send()
        .await?;
    assert_eq!(
        health.status().as_u16(),
        200,
        "live daemon must keep serving"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_second_http_launch_is_refused_before_touching_live_ledger() -> anyhow::Result<()>
{
    let bind = free_bind()?;
    assert_second_launch_leaves_live_daemon_untouched(&["--mode", "http", "--bind", &bind]).await
}

#[tokio::test]
async fn must_fail_second_stdio_launch_is_refused_before_touching_live_ledger() -> anyhow::Result<()>
{
    assert_second_launch_leaves_live_daemon_untouched(&["--mode", "stdio"]).await
}

#[tokio::test]
async fn must_pass_sole_lock_owner_runs_recovery_at_startup() -> anyhow::Result<()> {
    let home = tempfile::Builder::new()
        .prefix("synapse-g1b-170-")
        .tempdir()?;
    let db = home.path().join("db");
    std::fs::create_dir_all(&db)?;
    std::fs::write(db.join("action_recovery.jsonl"), POISON)?;
    assert!(
        !db.join("daemon.pid").exists(),
        "precondition: no lock holder"
    );
    let bind = free_bind()?;

    let command = daemon_command(home.path(), &db, &["--mode", "http", "--bind", &bind]);
    let (code, stderr) = run_to_exit(command).await?;

    assert_eq!(
        code,
        Some(1),
        "owner must run recovery and fail loudly; stderr={stderr}"
    );
    assert!(stderr.contains(RECOVERY_FAILURE), "stderr={stderr}");
    Ok(())
}
