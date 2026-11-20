//! Slow partial headers must not consume the liveness reserve either (#19 re-review).
//!
//! Fills the normal budget (8) AND floods well past it with incomplete headers; an
//! authenticated `/health` request must still be served, and a non-liveness request
//! that completes its headers while saturated must not be handed the reserve.

use std::{net::TcpListener, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};

const TOKEN: &str = "syn19-reserve-isolation-test-token";

#[tokio::test]
async fn must_fail_non_liveness_request_under_slow_header_flood_gets_no_reserve_slot()
-> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind = listener.local_addr()?.to_string();
    drop(listener);
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .args(["--mode", "http", "--bind", &bind, "--db"])
        .arg(directory.path().join("db"))
        .env("APPDATA", directory.path())
        .env("LOCALAPPDATA", directory.path())
        .env("SYNAPSE_LOG_DIR", directory.path())
        .env("SYNAPSE_BEARER_TOKEN", TOKEN)
        .env("SYNAPSE_HTTP_MAX_CONNECTIONS", "8")
        .env("SYNAPSE_HTTP_HEADER_READ_TIMEOUT_SECS", "4")
        .env("SYNAPSE_HTTP_LIVENESS_CONNECTION_RESERVE", "2")
        .env("SYNAPSE_HTTP_LIVENESS_HEADER_READ_TIMEOUT_SECS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated HTTP daemon")?;

    let result: anyhow::Result<()> = async {
        let client = reqwest::Client::builder().pool_max_idle_per_host(0).build()?;
        wait_for_health(&client, &bind).await?;
        // 8 fill the normal budget; 24 more would have taken the two reserve slots
        // (and everything beyond them was rejected) under the old design.
        let mut slow = Vec::new();
        let flood_started = tokio::time::Instant::now();
        for index in 0..32 {
            let mut stream = TcpStream::connect(&bind).await?;
            let head = if index % 2 == 0 {
                format!("GET /health HTTP/1.1\r\nHost: {bind}\r\n")
            } else {
                format!("GET /mcp HTTP/1.1\r\nHost: {bind}\r\n")
            };
            stream.write_all(head.as_bytes()).await?;
            slow.push(stream);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        let health = tokio::time::timeout(
            Duration::from_secs(3),
            client
                .get(format!("http://{bind}/health"))
                .bearer_auth(TOKEN)
                .send(),
        )
        .await
        .context("health request timed out under slow-header flood")?
        .context("health request dropped under slow-header flood")?;
        ensure!(
            health.status().is_success(),
            "health returned {} under slow-header flood",
            health.status()
        );

        // Non-liveness request with COMPLETE headers while saturated: must not use the reserve.
        let mut other = TcpStream::connect(&bind).await?;
        other
            .write_all(
                format!(
                    "GET /does-not-exist HTTP/1.1\r\nHost: {bind}\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let mut byte = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), other.read(&mut byte)).await;
        ensure!(
            matches!(read, Ok(Ok(0) | Err(_))),
            "non-liveness request was served from the liveness reserve while saturated"
        );

        // Poll each slow connection until a generous deadline past the 4 s
        // header timeout; it must be closed/reset, never still open.
        let close_deadline = flood_started + Duration::from_secs(9);
        for (index, stream) in slow.iter_mut().enumerate() {
            let read = tokio::time::timeout_at(close_deadline, stream.read(&mut byte)).await;
            ensure!(
                matches!(read, Ok(Ok(0) | Err(_))),
                "slow header connection {index} remained open past configured deadline: {read:?}"
            );
        }
        // Recovered: a plain health request works after the flood expired.
        let after = client
            .get(format!("http://{bind}/health"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .context("health after flood")?;
        ensure!(after.status().is_success(), "health failed after flood");
        Ok(())
    }
    .await;

    let _ = reqwest::Client::new()
        .post(format!("http://{bind}/shutdown"))
        .bearer_auth(TOKEN)
        .send()
        .await;
    if tokio::time::timeout(Duration::from_secs(10), daemon.wait())
        .await
        .is_err()
    {
        let _ = daemon.start_kill();
        let _ = daemon.wait().await;
    }
    result
}

#[tokio::test]
async fn must_pass_non_liveness_request_is_served_while_normal_budget_has_room()
-> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind = listener.local_addr()?.to_string();
    drop(listener);
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .args(["--mode", "http", "--bind", &bind, "--db"])
        .arg(directory.path().join("db"))
        .env("APPDATA", directory.path())
        .env("LOCALAPPDATA", directory.path())
        .env("SYNAPSE_LOG_DIR", directory.path())
        .env("SYNAPSE_BEARER_TOKEN", TOKEN)
        .env("SYNAPSE_HTTP_MAX_CONNECTIONS", "8")
        .env("SYNAPSE_HTTP_HEADER_READ_TIMEOUT_SECS", "4")
        .env("SYNAPSE_HTTP_LIVENESS_CONNECTION_RESERVE", "2")
        .env("SYNAPSE_HTTP_LIVENESS_HEADER_READ_TIMEOUT_SECS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated HTTP daemon")?;

    let result: anyhow::Result<()> = async {
        let client = reqwest::Client::builder().pool_max_idle_per_host(0).build()?;
        wait_for_health(&client, &bind).await?;
        // Half the normal budget is held by slow partial heads; the rest must stay usable
        // for ordinary (non-liveness) traffic.
        let mut slow = Vec::new();
        for _ in 0..4 {
            let mut stream = TcpStream::connect(&bind).await?;
            stream
                .write_all(format!("GET /mcp HTTP/1.1\r\nHost: {bind}\r\n").as_bytes())
                .await?;
            slow.push(stream);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;

        let mut other = TcpStream::connect(&bind).await?;
        other
            .write_all(
                format!(
                    "GET /does-not-exist HTTP/1.1\r\nHost: {bind}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), other.read_to_end(&mut response))
            .await
            .context("non-liveness request timed out with budget available")??;
        let response = String::from_utf8_lossy(&response);
        ensure!(
            response.starts_with("HTTP/1.1 404"),
            "non-liveness request was not served normally with budget available: {response:?}"
        );

        let health = tokio::time::timeout(
            Duration::from_secs(3),
            client
                .get(format!("http://{bind}/health"))
                .bearer_auth(TOKEN)
                .send(),
        )
        .await
        .context("health timed out with budget available")?
        .context("health dropped with budget available")?;
        ensure!(
            health.status().is_success(),
            "health returned {} with budget available",
            health.status()
        );
        drop(slow);
        Ok(())
    }
    .await;

    let _ = reqwest::Client::new()
        .post(format!("http://{bind}/shutdown"))
        .bearer_auth(TOKEN)
        .send()
        .await;
    if tokio::time::timeout(Duration::from_secs(10), daemon.wait())
        .await
        .is_err()
    {
        let _ = daemon.start_kill();
        let _ = daemon.wait().await;
    }
    result
}

async fn wait_for_health(client: &reqwest::Client, bind: &str) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(response) = client
            .get(format!("http://{bind}/health"))
            .bearer_auth(TOKEN)
            .send()
            .await
            && response.status().is_success()
        {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "daemon health timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
