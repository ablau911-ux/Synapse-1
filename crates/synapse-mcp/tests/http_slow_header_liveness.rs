//! Slow partial headers must not consume every health-check connection (#19).

use std::{net::TcpListener, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};

const TOKEN: &str = "syn19-slow-header-test-token";

#[tokio::test]
async fn must_fail_slow_partial_headers_are_cut_at_header_deadline() -> anyhow::Result<()> {
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
        .env("SYNAPSE_HTTP_HEADER_READ_TIMEOUT_SECS", "3")
        .env("SYNAPSE_HTTP_LIVENESS_CONNECTION_RESERVE", "2")
        .env("SYNAPSE_HTTP_LIVENESS_HEADER_READ_TIMEOUT_SECS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated HTTP daemon")?;

    let result: anyhow::Result<()> = async {
        let client = reqwest::Client::new();
        wait_for_health(&client, &bind).await?;
        let mut slow = Vec::new();
        let slow_started = tokio::time::Instant::now();
        for _ in 0..8 {
            let mut stream = TcpStream::connect(&bind).await?;
            stream
                .write_all(format!("GET /health HTTP/1.1\r\nHost: {bind}\r\n").as_bytes())
                .await?;
            slow.push(stream);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        let health = tokio::time::timeout(
            Duration::from_secs(2),
            client
                .get(format!("http://{bind}/health"))
                .bearer_auth(TOKEN)
                .send(),
        )
        .await??;
        ensure!(
            health.status().is_success(),
            "health failed under slow headers"
        );

        // Each slow connection must be closed by the server once the 3 s
        // header deadline passes: poll until a generous deadline instead of
        // sleeping a fixed time, and require a close/reset, never a timeout.
        let close_deadline = slow_started + Duration::from_secs(8);
        for (index, stream) in slow.iter_mut().enumerate() {
            let mut byte = [0_u8; 1];
            let read = tokio::time::timeout_at(close_deadline, stream.read(&mut byte)).await;
            ensure!(
                matches!(read, Ok(Ok(0) | Err(_))),
                "slow header connection {index} remained open past configured deadline: {read:?}"
            );
            let closed_after = slow_started.elapsed();
            ensure!(
                closed_after >= Duration::from_secs(2),
                "slow header connection {index} was cut after {closed_after:?}, before the 3 s header deadline"
            );
        }
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
async fn must_pass_headers_completed_within_read_timeout_are_served() -> anyhow::Result<()> {
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
        .env("SYNAPSE_HTTP_HEADER_READ_TIMEOUT_SECS", "3")
        .env("SYNAPSE_HTTP_LIVENESS_CONNECTION_RESERVE", "2")
        .env("SYNAPSE_HTTP_LIVENESS_HEADER_READ_TIMEOUT_SECS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated HTTP daemon")?;

    let result: anyhow::Result<()> = async {
        let client = reqwest::Client::new();
        wait_for_health(&client, &bind).await?;
        // A slow but in-time client: the head arrives in pieces, finishing in
        // ~600 ms: inside BOTH the 1 s liveness header timeout and the 3 s
        // normal one, so it must be served whichever path the connection takes.
        let mut stream = TcpStream::connect(&bind).await?;
        let pieces = [
            "GET /health HTTP/1.1\r\n".to_owned(),
            format!("Host: {bind}\r\n"),
            format!("Authorization: Bearer {TOKEN}\r\n"),
            "Connection: close\r\n\r\n".to_owned(),
        ];
        for piece in &pieces {
            stream.write_all(piece.as_bytes()).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut response))
            .await
            .context("in-time slow head was not answered")??;
        let response = String::from_utf8_lossy(&response);
        ensure!(
            response.starts_with("HTTP/1.1 200"),
            "in-time slow head was not served: {response:?}"
        );
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
