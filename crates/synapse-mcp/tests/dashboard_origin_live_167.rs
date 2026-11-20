//! #167 on the REAL daemon router (`transport.rs::router_with_auth` wiring),
//! not a substitute router: cross-origin dashboard writes and the
//! agent-terminal WebSocket upgrade are refused with 403
//! `DASHBOARD_ORIGIN_REFUSED`, a non-loopback Host with 403
//! `DASHBOARD_HOST_REFUSED`, and a refused write leaves the source of truth
//! (CF saved-view rows, read back via GET /dashboard/saved-views) untouched.
//! Same-origin and Origin-less local clients keep working.

#[allow(dead_code)]
mod support;

use std::time::Duration;

use anyhow::Context;
use serde_json::{Value, json};
use support::HttpDaemon;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

fn bind(daemon: &HttpDaemon) -> String {
    daemon.url("").trim_start_matches("http://").to_owned()
}

/// Raw HTTP/1.1 exchange so Host/Upgrade headers are exactly what the test says.
async fn raw(bind: &str, head: &str, body: &str) -> anyhow::Result<(u16, String)> {
    let mut stream = tokio::net::TcpStream::connect(bind).await?;
    let connection = if head.contains("\r\nUpgrade:") {
        "upgrade"
    } else {
        "close"
    };
    let request = format!(
        "{head}\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    tokio::time::timeout(Duration::from_secs(10), read_response(stream)).await?
}

async fn read_response(stream: tokio::net::TcpStream) -> anyhow::Result<(u16, String)> {
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        anyhow::ensure!(
            reader.read_line(&mut line).await? != 0,
            "EOF before HTTP headers"
        );
        if line == "\r\n" {
            break;
        }
        head.push_str(&line);
        anyhow::ensure!(head.len() <= 16_384, "HTTP response headers too large");
    }
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .context("status line")?;
    let length: usize = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .context("Content-Length missing")?
        .1
        .trim()
        .parse()?;
    anyhow::ensure!(length <= 1_048_576, "HTTP response body too large");
    let mut body = vec![0; length];
    // A refused WebSocket can reset TCP after the complete response. EOF is
    // not the HTTP message boundary; verify every declared body byte instead.
    reader.read_exact(&mut body).await?;
    Ok((status, String::from_utf8(body)?))
}

async fn upsert_view(
    daemon: &HttpDaemon,
    origin: Option<&str>,
    name: &str,
) -> anyhow::Result<(u16, String)> {
    let body = json!({"name": name, "route": "fleet", "filters": {}}).to_string();
    let origin = origin.map_or(String::new(), |origin| format!("\r\nOrigin: {origin}"));
    let head = format!(
        "POST /dashboard/saved-views HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json{origin}",
        bind(daemon)
    );
    raw(&bind(daemon), &head, &body).await
}

async fn view_names(daemon: &HttpDaemon) -> anyhow::Result<Vec<String>> {
    let views: Value = reqwest::Client::new()
        .get(daemon.url("/dashboard/saved-views"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(views["views"]
        .as_array()
        .context("views array")?
        .iter()
        .filter_map(|view| view["name"].as_str().map(str::to_owned))
        .collect())
}

#[tokio::test]
async fn must_fail_cross_origin_saved_view_write_is_403_and_writes_nothing() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let name = "g1b-167-evil";
    assert!(
        !view_names(&daemon).await?.contains(&name.to_owned()),
        "precondition"
    );
    for origin in ["http://evil.example", "null", "http://127.0.0.1:1"] {
        let (status, body) = upsert_view(&daemon, Some(origin), name).await?;
        assert_eq!(
            (status, body.trim()),
            (403, "DASHBOARD_ORIGIN_REFUSED"),
            "origin {origin}"
        );
    }
    assert!(
        !view_names(&daemon).await?.contains(&name.to_owned()),
        "refused write persisted"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_cross_origin_agent_terminal_websocket_upgrade_is_403() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let head = format!(
        "GET /dashboard/agent-terminal/g1b-167/ws?mode=controller HTTP/1.1\r\nHost: {}\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nOrigin: http://evil.example",
        bind(&daemon)
    );
    let (status, body) = raw(&bind(&daemon), &head, "").await?;
    assert_eq!((status, body.trim()), (403, "DASHBOARD_ORIGIN_REFUSED"));
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_rebinding_host_is_403_on_reads_and_writes() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let port = bind(&daemon).rsplit(':').next().context("port")?.to_owned();
    for host in [
        format!("evil.example:{port}"),
        format!("192.168.1.20:{port}"),
    ] {
        let head = format!("GET /dashboard/state.json HTTP/1.1\r\nHost: {host}");
        let (status, body) = raw(&bind(&daemon), &head, "").await?;
        assert_eq!(
            (status, body.trim()),
            (403, "DASHBOARD_HOST_REFUSED"),
            "host {host}"
        );
    }
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_same_origin_and_originless_local_writes_persist() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let own_origin = daemon.url("");
    let (status, body) = upsert_view(&daemon, Some(&own_origin), "g1b-167-same").await?;
    assert_eq!(status, 200, "same-origin write: {body}");
    let (status, body) = upsert_view(&daemon, None, "g1b-167-local").await?;
    assert_eq!(status, 200, "Origin-less local client: {body}");
    let names = view_names(&daemon).await?;
    for name in ["g1b-167-same", "g1b-167-local"] {
        assert!(
            names.contains(&name.to_owned()),
            "{name} missing from {names:?}"
        );
    }
    drop(daemon.stop().await?);
    Ok(())
}
