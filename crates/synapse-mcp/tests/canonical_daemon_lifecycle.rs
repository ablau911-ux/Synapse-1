//! The setup launcher arguments must become healthy and leave truthful lifecycle state (#64).

use std::{net::TcpListener, path::Path, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use serde_json::Value;
use tokio::process::{Child, Command};

const TOKEN: &str = "syn64-canonical-launch-test-token";

#[tokio::test]
async fn must_pass_canonical_launcher_health_and_restart_match_lifecycle() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let db = directory.path().join("db-daemon");
    let profiles = directory.path().join("profiles");
    std::fs::create_dir_all(&profiles)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind = listener.local_addr()?.to_string();
    drop(listener);

    let mut first = launch(directory.path(), &db, &profiles, &bind)?;
    let first_pid = first.id().context("first daemon PID")?;
    let first_result: anyhow::Result<()> = async {
        let health = wait_for_health(&bind).await?;
        verify_current_run(&db, &health, first_pid)?;
        shutdown(&bind).await?;
        tokio::time::timeout(Duration::from_secs(10), first.wait()).await??;
        let ended = read_run(&db)?;
        ensure!(
            ended["ended_at_unix_ms"]
                .as_u64()
                .is_some_and(|value| value > 0),
            "first run did not finalize: {ended}"
        );
        Ok(())
    }
    .await;
    if first_result.is_err() {
        let _ = first.start_kill();
        let _ = first.wait().await;
    }
    first_result?;

    let mut second = launch(directory.path(), &db, &profiles, &bind)?;
    let second_result: anyhow::Result<()> = async {
        let health = wait_for_health(&bind).await?;
        let second_pid = second.id().context("second daemon PID")?;
        verify_current_run(&db, &health, second_pid)?;
        ensure!(second_pid != first_pid, "restart reused first PID");
        Ok(())
    }
    .await;
    let stop_result = shutdown(&bind).await;
    let wait_result = tokio::time::timeout(Duration::from_secs(10), second.wait()).await;
    if second_result.is_err() || stop_result.is_err() || wait_result.is_err() {
        let _ = second.start_kill();
        let _ = second.wait().await;
    }
    second_result?;
    stop_result?;
    wait_result??;
    Ok(())
}

#[tokio::test]
async fn must_fail_unauthenticated_shutdown_leaves_lifecycle_running() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let db = directory.path().join("db-daemon");
    let profiles = directory.path().join("profiles");
    std::fs::create_dir_all(&profiles)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind = listener.local_addr()?.to_string();
    drop(listener);

    let mut daemon = launch(directory.path(), &db, &profiles, &bind)?;
    let result: anyhow::Result<()> = async {
        let pid = daemon.id().context("daemon PID")?;
        let health = wait_for_health(&bind).await?;
        verify_current_run(&db, &health, pid)?;
        let client = reqwest::Client::new();
        for bearer in [None, Some("syn64-wrong-token"), Some("")] {
            let mut request = client.post(format!("http://{bind}/shutdown"));
            if let Some(token) = bearer {
                request = request.bearer_auth(token);
            }
            let response = request.send().await?;
            let status = response.status();
            let body = response.text().await?;
            ensure!(
                status == reqwest::StatusCode::UNAUTHORIZED && body == "HTTP_TOKEN_INVALID",
                "shutdown with bearer {bearer:?} was not refused: {status} {body}"
            );
        }
        let unauthenticated_health = client.get(format!("http://{bind}/health")).send().await?;
        ensure!(
            unauthenticated_health.status() == reqwest::StatusCode::UNAUTHORIZED,
            "unauthenticated health was served: {}",
            unauthenticated_health.status()
        );
        ensure!(
            daemon.try_wait()?.is_none(),
            "daemon exited after refused shutdown"
        );
        let health = wait_for_health(&bind).await?;
        verify_current_run(&db, &health, pid)?;
        Ok(())
    }
    .await;
    let stop_result = shutdown(&bind).await;
    let wait_result = tokio::time::timeout(Duration::from_secs(10), daemon.wait()).await;
    if result.is_err() || stop_result.is_err() || wait_result.is_err() {
        let _ = daemon.start_kill();
        let _ = daemon.wait().await;
    }
    result?;
    stop_result?;
    wait_result??;
    Ok(())
}

fn launch(directory: &Path, db: &Path, profiles: &Path, bind: &str) -> anyhow::Result<Child> {
    Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .args([
            "--mode",
            "http",
            "--bind",
            bind,
            "--db",
            &db.to_string_lossy(),
            "--profile-dir",
            &profiles.to_string_lossy(),
            "--log-level",
            "info",
        ])
        .env("APPDATA", directory)
        .env("LOCALAPPDATA", directory)
        .env("SYNAPSE_LOG_DIR", directory)
        .env("SYNAPSE_BEARER_TOKEN", TOKEN)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start canonical isolated daemon")
}

async fn wait_for_health(bind: &str) -> anyhow::Result<Value> {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(response) = client
            .get(format!("http://{bind}/health"))
            .bearer_auth(TOKEN)
            .send()
            .await
            && response.status().is_success()
        {
            return response.json().await.context("decode daemon health");
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "canonical daemon did not become healthy"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn shutdown(bind: &str) -> anyhow::Result<()> {
    let response = reqwest::Client::new()
        .post(format!("http://{bind}/shutdown"))
        .bearer_auth(TOKEN)
        .send()
        .await?;
    ensure!(response.status().is_success(), "shutdown HTTP failure");
    Ok(())
}

fn read_run(db: &Path) -> anyhow::Result<Value> {
    serde_json::from_slice(&std::fs::read(db.join("daemon-run-current.json"))?)
        .context("decode canonical lifecycle record")
}

fn verify_current_run(db: &Path, health: &Value, pid: u32) -> anyhow::Result<()> {
    let run = read_run(db)?;
    ensure!(
        health["ok"] == true
            && health["pid"] == pid
            && run["pid"] == pid
            && run["mode"] == "http"
            && run["db_path"] == db.to_string_lossy().as_ref()
            && run["started_at_unix_ms"]
                .as_u64()
                .is_some_and(|value| value > 0)
            && run["ended_at_unix_ms"].is_null(),
        "health/lifecycle mismatch: health={health} run={run}"
    );
    Ok(())
}
