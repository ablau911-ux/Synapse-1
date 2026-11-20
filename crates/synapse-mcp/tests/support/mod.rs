//! Shared harness for integration tests that need the real Streamable HTTP
//! daemon (per-session tool profiles, dashboard routes).
//!
//! Every `tools/call` goes through the same client-parity checks as the stdio
//! client (D1, #548/#549): `tools/list` is validated like a strict client
//! validates it, arguments are checked against the listed `inputSchema`
//! before send, `isError: true` is a failure, and `structuredContent` must
//! satisfy `outputSchema`. `tools_call_error` is the explicit server-side
//! refusal probe and returns the JSON-RPC `error` object.

use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use synapse_test_utils::tool_catalog::{ToolCatalog, is_tool_error};
use tokio::process::{Child, Command};

const PROTOCOL_VERSION: &str = "2025-06-18";
const BEARER: &str = "synapse-tests-http-bearer";

#[path = "restart.rs"]
mod restart;

/// Clean-environment `synapse-mcp --mode http` on a loopback port with its
/// DB, logs, APPDATA and LOCALAPPDATA under one temp home.
pub struct HttpDaemon {
    child: Child,
    bind: String,
    db_path: PathBuf,
    home: tempfile::TempDir,
}

impl HttpDaemon {
    /// Starts the daemon with `envs` layered over the isolated defaults and
    /// waits for an authenticated `/health` 200.
    pub async fn start(envs: &[(&str, &str)]) -> anyhow::Result<Self> {
        let home = tempfile::Builder::new()
            .prefix("synapse-http-home-")
            .tempdir()?;
        let db_path = home.path().join("db");
        let bind = free_loopback_bind()?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"));
        for (key, _value) in std::env::vars_os() {
            if key
                .to_str()
                .is_some_and(|key| key.to_ascii_uppercase().starts_with("SYNAPSE_"))
            {
                command.env_remove(&key);
            }
        }
        command
            .args(["--mode", "http", "--bind", &bind, "--db"])
            .arg(&db_path)
            .env("APPDATA", home.path())
            .env("LOCALAPPDATA", home.path())
            .env("SYNAPSE_LOG_DIR", home.path().join("logs"))
            .env("SYNAPSE_BEARER_TOKEN", BEARER)
            .env("SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY", "1")
            // #258: unrelated tests must not depend on the host's human activity
            // (a test daemon has no input recorder); `envs` may override it.
            .env("SYNAPSE_USER_ACTIVE_QUIET_MS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (key, value) in envs {
            command.env(key, value);
        }
        let child = command
            .spawn()
            .context("start isolated synapse-mcp http daemon")?;
        let daemon = Self {
            child,
            bind,
            db_path,
            home,
        };
        daemon.wait_for_health().await?;
        Ok(daemon)
    }

    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    #[must_use]
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.bind)
    }

    /// Opens one MCP session. `extra_headers` go on the `initialize` request
    /// only (e.g. the local-agent capability proof).
    pub async fn session(
        &self,
        client_name: &str,
        extra_headers: &[(&str, &str)],
    ) -> anyhow::Result<HttpMcpSession> {
        HttpMcpSession::connect(self.url("/mcp"), client_name, extra_headers).await
    }

    /// Kills the daemon and waits for exit so its RocksDB lock is released
    /// for a direct physical readback.
    pub async fn stop(mut self) -> anyhow::Result<tempfile::TempDir> {
        let _kill = self.child.start_kill();
        tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .context("timed out stopping isolated daemon")?
            .context("wait for isolated daemon exit")?;
        Ok(self.home)
    }

    async fn wait_for_health(&self) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let url = self.url("/health");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let result = client.get(&url).bearer_auth(BEARER).send().await;
            if result
                .as_ref()
                .is_ok_and(|response| response.status().is_success())
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                match result {
                    Ok(response) => bail!("daemon health returned {}", response.status()),
                    Err(error) => return Err(error).context("daemon health never answered"),
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// One Streamable HTTP MCP session with production-client parity checks.
pub struct HttpMcpSession {
    endpoint: String,
    session_id: String,
    client: reqwest::Client,
    request_id: AtomicU64,
    catalog: Option<ToolCatalog>,
}

impl HttpMcpSession {
    async fn connect(
        endpoint: String,
        client_name: &str,
        extra_headers: &[(&str, &str)],
    ) -> anyhow::Result<Self> {
        let client = reqwest::Client::new();
        let mut initialize = client
            .post(&endpoint)
            .bearer_auth(BEARER)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": client_name, "version": "1.0"}
                }
            }));
        for (name, value) in extra_headers {
            initialize = initialize.header(*name, *value);
        }
        let response = initialize.send().await.context("send initialize")?;
        let status = response.status();
        let session_id = response
            .headers()
            .get("mcp-session-id")
            .context("initialize response missing Mcp-Session-Id")?
            .to_str()
            .context("decode Mcp-Session-Id")?
            .to_owned();
        let body = decode_mcp_response(response).await?;
        ensure!(
            status.is_success() && body.get("error").is_none(),
            "initialize failed: status={status} body={body}"
        );
        let session = Self {
            endpoint,
            session_id,
            client,
            request_id: AtomicU64::new(2),
            catalog: None,
        };
        let initialized = session
            .post(json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}))
            .await?;
        ensure!(
            initialized.status().is_success(),
            "initialized notification failed: {}",
            initialized.status()
        );
        Ok(session)
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// `tools/list` validated like a strict client; refreshes the catalog.
    pub async fn tools_list(&mut self) -> anyhow::Result<Value> {
        let result = self.rpc("tools/list", json!({})).await?;
        let body = result
            .get("result")
            .cloned()
            .with_context(|| format!("tools/list missing result: {result}"))?;
        self.catalog = Some(ToolCatalog::from_tools_list(&body)?);
        Ok(body)
    }

    /// Schema-validated call that must succeed (no JSON-RPC error, no
    /// `isError`, `structuredContent` valid against `outputSchema`).
    pub async fn tools_call(&mut self, name: &str, args: Value) -> anyhow::Result<Value> {
        self.ensure_listed(name, &args).await?;
        let response = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        if let Some(error) = response.get("error") {
            bail!("tools/call `{name}` JSON-RPC error: {error}");
        }
        let result = response
            .get("result")
            .cloned()
            .with_context(|| format!("tools/call `{name}` missing result: {response}"))?;
        if is_tool_error(&result) {
            bail!("tools/call `{name}` returned isError=true: {result}");
        }
        self.catalog
            .as_ref()
            .context("tool catalog missing")?
            .check_structured_output(name, &result)?;
        Ok(result)
    }

    /// Schema-valid call whose result the test expects to be a tool-level
    /// failure (`isError: true`); returns that result. Any other outcome,
    /// including a JSON-RPC error, is a failure.
    pub async fn tools_call_expect_tool_error(
        &mut self,
        name: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        self.ensure_listed(name, &args).await?;
        let response = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        if let Some(error) = response.get("error") {
            bail!("tools/call `{name}` JSON-RPC error: {error}");
        }
        let result = response
            .get("result")
            .cloned()
            .with_context(|| format!("tools/call `{name}` missing result: {response}"))?;
        if !is_tool_error(&result) {
            bail!("tools/call `{name}` was expected to return isError=true, got: {result}");
        }
        Ok(result)
    }

    /// Schema-valid call the test expects the daemon to refuse with a
    /// JSON-RPC error; returns that `error` object. Success is a failure.
    pub async fn tools_call_error(&mut self, name: &str, args: Value) -> anyhow::Result<Value> {
        self.ensure_listed(name, &args).await?;
        let response = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        response
            .get("error")
            .cloned()
            .with_context(|| format!("tools/call `{name}` was expected to fail: {response}"))
    }

    /// Like [`Self::tools_call_error`], but sends arguments a strict client
    /// would refuse before sending, so a test observes the daemon's own
    /// refusal of them. Returns the JSON-RPC `error` object.
    pub async fn tools_call_error_unchecked(
        &self,
        name: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        let response = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        response
            .get("error")
            .cloned()
            .with_context(|| format!("tools/call `{name}` was expected to fail: {response}"))
    }

    async fn ensure_listed(&mut self, name: &str, args: &Value) -> anyhow::Result<()> {
        if !self
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.contains(name))
        {
            self.tools_list().await?;
        }
        self.catalog
            .as_ref()
            .context("tool catalog missing")?
            .check_arguments(name, args)
    }

    async fn rpc(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let response = self
            .post(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        let status = response.status();
        let body = decode_mcp_response(response).await?;
        ensure!(status.is_success(), "MCP {method} HTTP {status}: {body}");
        Ok(body)
    }

    async fn post(&self, body: Value) -> anyhow::Result<reqwest::Response> {
        self.client
            .post(&self.endpoint)
            .bearer_auth(BEARER)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header("Mcp-Session-Id", &self.session_id)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(&body)
            .send()
            .await
            .context("send MCP request")
    }
}

/// Decodes a JSON or SSE-framed JSON-RPC response body.
pub async fn decode_mcp_response(response: reqwest::Response) -> anyhow::Result<Value> {
    let body = response.text().await.context("read MCP response")?;
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).context("decode JSON MCP response");
    }
    let normalized = trimmed.replace("\r\n", "\n");
    for frame in normalized.split("\n\n") {
        let data = frame
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.starts_with('{') {
            return serde_json::from_str(&data).context("decode SSE MCP response");
        }
    }
    bail!("MCP response did not contain a JSON-RPC frame: {body}")
}

fn free_loopback_bind() -> anyhow::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").context("allocate loopback port")?;
    let bind = listener.local_addr()?.to_string();
    drop(listener);
    Ok(bind)
}
