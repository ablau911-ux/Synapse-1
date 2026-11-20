//! #182: production router, not a miniature router with a test-defined limit.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(dead_code)]
mod support;

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use support::HttpDaemon;

const BEARER: &str = "synapse-tests-http-bearer";
const LIMIT: usize = 64 * 1024 * 1024;

async fn register(daemon: &HttpDaemon, client: &Client) -> anyhow::Result<String> {
    let response = client
        .post(daemon.url("/chrome-debugger/native/register"))
        .bearer_auth(BEARER)
        .json(&json!({
            "origin": "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/",
            "pid": std::process::id(), "parent_window": null,
            "bridge_protocol_version": 1, "transport": "native_messaging"
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["ok"], true);
    Ok(body["host_id"]
        .as_str()
        .expect("registered host")
        .to_owned())
}

async fn post(
    daemon: &HttpDaemon,
    client: &Client,
    host: &str,
    size: usize,
) -> anyhow::Result<reqwest::Response> {
    let body = json!({"host_id": host, "message": {
        "type": "response", "id": "unmatched-synthetic-182", "ok": true,
        "result": {"data": "x".repeat(size)}, "error": null
    }});
    Ok(client
        .post(daemon.url("/chrome-debugger/native/message"))
        .bearer_auth(BEARER)
        .json(&body)
        .send()
        .await?)
}

async fn read_host(daemon: &HttpDaemon, client: &Client, host: &str) -> anyhow::Result<()> {
    let url = daemon.url(&format!(
        "/chrome-debugger/native/next?host_id={host}&timeout_ms=1"
    ));
    let response = client.get(url).bearer_auth(BEARER).send().await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "registered host must remain usable"
    );
    Ok(())
}

#[tokio::test]
async fn must_pass_three_mib_response_reaches_production_message_handler() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let client = Client::new();
    let host = register(&daemon, &client).await?;
    read_host(&daemon, &client, &host).await?;
    let response = post(&daemon, &client, &host, 3 * 1024 * 1024).await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "regression: Axum default rejected >2 MiB"
    );
    assert_eq!(response.json::<Value>().await?, json!({"ok": true}));
    read_host(&daemon, &client, &host).await?;
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_over_limit_and_malformed_bodies_preserve_registered_host() -> anyhow::Result<()>
{
    let daemon = HttpDaemon::start(&[]).await?;
    let client = Client::new();
    let host = register(&daemon, &client).await?;
    read_host(&daemon, &client, &host).await?;
    let response = post(&daemon, &client, &host, LIMIT).await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "CHROME_BRIDGE_MESSAGE_REJECTED");
    assert_eq!(body["max_bytes"], LIMIT);
    read_host(&daemon, &client, &host).await?;
    let bad = client
        .post(daemon.url("/chrome-debugger/native/message"))
        .bearer_auth(BEARER)
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await?;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        bad.json::<Value>().await?["code"],
        "CHROME_BRIDGE_MESSAGE_REJECTED"
    );
    read_host(&daemon, &client, &host).await?;
    assert_eq!(
        post(&daemon, &client, &host, 0).await?.status(),
        StatusCode::OK
    );
    drop(daemon.stop().await?);
    Ok(())
}
