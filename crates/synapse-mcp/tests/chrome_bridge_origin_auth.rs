use std::{net::TcpListener, path::Path, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use serde_json::Value;
use tokio::process::{Child, Command};

const BEARER: &str = "syn30-isolated-bearer";
const ORIGIN: &str = "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk";
const REGISTRATION: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn must_fail_forged_origin_wrong_token_and_cross_host_token_are_refused() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    let (daemon, base) = start_daemon(directory.path()).await?;
    let result = async {
        let client = reqwest::Client::new();
        // Contract (docs/systemdocs/15_mcp_server_architecture.md §4.3,
        // src/http/auth.rs `require_http_security`): the extension carve-out
        // applies only when the bridge token (or registration token) matches
        // (src/chrome_debugger_bridge.rs
        // `direct_http_extension_bridge_request_with_token_dir`); otherwise
        // the origin check runs FIRST and a `chrome-extension://` Origin is
        // refused with 403 `HTTP_ORIGIN_REFUSED` (auth.rs `forbidden`)
        // before any bearer or handler check.
        let response = client
            .get(format!(
                "{base}/chrome-debugger/native/next?host_id=unknown"
            ))
            .header("Origin", ORIGIN)
            .send()
            .await?;
        assert_origin_refused(response, "forged Origin on next").await?;
        let response = client
            .post(format!("{base}/chrome-debugger/native/register"))
            .header("Origin", ORIGIN)
            .json(&registration_body())
            .send()
            .await?;
        assert_origin_refused(response, "forged Origin on register").await?;

        // With a registration token on disk, a wrong token is still refused.
        write_registration_token(directory.path())?;
        anyhow::ensure!(
            directory
                .path()
                .join("synapse/chrome-bridge-token.txt")
                .is_file(),
            "fixture precondition: registration token file must exist"
        );
        let response = register_with(&client, &base, "wrong").await?;
        assert_origin_refused(response, "wrong registration token").await?;

        // One host's bridge token never reaches another host. Through the
        // extension path the carve-out's host-token check fails, so the
        // origin layer refuses it (403 HTTP_ORIGIN_REFUSED).
        let first = registered_host(&client, &base).await?;
        let second = registered_host(&client, &base).await?;
        let response = next_for(&client, &base, &second.0, &first.1).await?;
        assert_origin_refused(response, "cross-host token via extension origin").await?;

        // Handler layer: an authorized outer transport (valid bearer, no
        // Origin) reaches `http_next`, whose host-token check refuses the
        // other host's token with 401 HTTP_TOKEN_INVALID
        // (chrome_debugger_bridge.rs `bridge_host_token_rejected`).
        let response = client
            .get(format!(
                "{base}/chrome-debugger/native/next?host_id={}&timeout_ms=0",
                second.0
            ))
            .bearer_auth(BEARER)
            .header("X-Synapse-Bridge-Token", &first.1)
            .send()
            .await?;
        ensure!(
            response.status().as_u16() == 401,
            "handler must refuse the other host's token with 401: {}",
            response.status()
        );
        let body: Value = response.json().await?;
        ensure!(
            body["code"] == synapse_core::error_codes::HTTP_TOKEN_INVALID,
            "cross-host token refusal must be HTTP_TOKEN_INVALID: {body}"
        );
        // The cross-host attempt did not rebind the second host: its own
        // token still works for it.
        let own = next_for(&client, &base, &second.0, &second.1).await?;
        ensure!(
            own.status().is_success(),
            "second host's own token must still work after the refusal: {}",
            own.status()
        );
        Ok(())
    }
    .await;
    stop_daemon(daemon, &base).await;
    result
}

#[tokio::test]
async fn must_pass_valid_registrations_get_unique_hosts_and_own_tokens_work() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    let (daemon, base) = start_daemon(directory.path()).await?;
    let result = async {
        let client = reqwest::Client::new();
        write_registration_token(directory.path())?;
        let first = registered_host(&client, &base).await?;
        ensure!(
            first
                .0
                .strip_prefix("chrome-native-")
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .is_some(),
            "host id is predictable or malformed: {}",
            first.0
        );
        let second = registered_host(&client, &base).await?;
        ensure!(first.0 != second.0, "host ids collided");

        let response = next_for(&client, &base, &second.0, &second.1).await?;
        ensure!(
            response.status().is_success(),
            "own host token denied: {}",
            response.status()
        );

        let response = client
            .post(format!("{base}/chrome-debugger/native/register"))
            .bearer_auth(BEARER)
            .json(&serde_json::json!({"origin":format!("{ORIGIN}/"),"pid":1,
                "parent_window":null,"bridge_protocol_version":1,"transport":"native_messaging"}))
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "native bearer registration failed: {}",
            response.status()
        );
        Ok(())
    }
    .await;
    stop_daemon(daemon, &base).await;
    result
}

fn registration_body() -> Value {
    serde_json::json!({"origin":format!("{ORIGIN}/"),"pid":0,
        "parent_window":null,"bridge_protocol_version":1,"transport":"direct_http"})
}

async fn assert_origin_refused(response: reqwest::Response, case: &str) -> anyhow::Result<()> {
    let status = response.status();
    let body = response.text().await?;
    ensure!(
        status.as_u16() == 403 && body == synapse_core::error_codes::HTTP_ORIGIN_REFUSED,
        "{case}: expected 403 {}, got {status} {body:?}",
        synapse_core::error_codes::HTTP_ORIGIN_REFUSED
    );
    Ok(())
}

fn write_registration_token(directory: &Path) -> anyhow::Result<()> {
    let registration_path = directory.join("synapse/chrome-bridge-token.txt");
    std::fs::create_dir_all(registration_path.parent().context("token parent")?)?;
    std::fs::write(&registration_path, REGISTRATION)?;
    Ok(())
}

async fn register_with(
    client: &reqwest::Client,
    base: &str,
    token: &str,
) -> anyhow::Result<reqwest::Response> {
    Ok(client
        .post(format!("{base}/chrome-debugger/native/register"))
        .header("Origin", ORIGIN)
        .header("X-Synapse-Bridge-Registration-Token", token)
        .json(&registration_body())
        .send()
        .await?)
}

/// Registers a direct-HTTP bridge host; returns `(host_id, bridge_token)`.
async fn registered_host(client: &reqwest::Client, base: &str) -> anyhow::Result<(String, String)> {
    let response = register_with(client, base, REGISTRATION).await?;
    ensure!(
        response.status().is_success(),
        "valid bridge registration failed: {}",
        response.status()
    );
    let body: Value = response.json().await?;
    let host = body["host_id"].as_str().context("host id")?.to_owned();
    let token = body["bridge_token"]
        .as_str()
        .context("bridge token")?
        .to_owned();
    Ok((host, token))
}

async fn next_for(
    client: &reqwest::Client,
    base: &str,
    host_id: &str,
    bridge_token: &str,
) -> anyhow::Result<reqwest::Response> {
    Ok(client
        .get(format!(
            "{base}/chrome-debugger/native/next?host_id={host_id}&timeout_ms=0"
        ))
        .header("Origin", ORIGIN)
        .header("X-Synapse-Bridge-Token", bridge_token)
        .send()
        .await?)
}

async fn start_daemon(directory: &Path) -> anyhow::Result<(Child, String)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind = listener.local_addr()?.to_string();
    drop(listener);
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .args(["--mode", "http", "--bind", &bind, "--db"])
        .arg(directory.join("db"))
        .env("APPDATA", directory)
        .env("LOCALAPPDATA", directory)
        .env("SYNAPSE_LOG_DIR", directory)
        .env("SYNAPSE_BEARER_TOKEN", BEARER)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated daemon")?;
    let base = format!("http://{bind}");
    if let Err(error) = wait_healthy(&base).await {
        let _ = daemon.start_kill();
        let _ = daemon.wait().await;
        return Err(error);
    }
    Ok((daemon, base))
}

async fn wait_healthy(base: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(response) = client
            .get(format!("{base}/health"))
            .bearer_auth(BEARER)
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

async fn stop_daemon(mut daemon: Child, base: &str) {
    let _ = reqwest::Client::new()
        .post(format!("{base}/shutdown"))
        .bearer_auth(BEARER)
        .send()
        .await;
    if tokio::time::timeout(Duration::from_secs(10), daemon.wait())
        .await
        .is_err()
    {
        let _ = daemon.start_kill();
        let _ = daemon.wait().await;
    }
}
