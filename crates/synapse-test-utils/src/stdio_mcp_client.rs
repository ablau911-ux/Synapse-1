use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, bail};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
};

use crate::tool_catalog::{ToolCatalog, is_tool_error};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// JSON-RPC stdio MCP client with production-client parity (#548/#549):
/// `tools/list` is checked like a strict client checks it, and `tools_call`
/// validates arguments against the listed `inputSchema` before send.
pub struct StdioMcpClient {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr_task: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr_buffer: Arc<Mutex<Vec<u8>>>,
    _scratch_dir: tempfile::TempDir,
    catalog: Option<ToolCatalog>,
    next_id: u64,
    raw_rx: Vec<String>,
    raw_tx: Vec<String>,
}

impl StdioMcpClient {
    pub async fn launch_and_init() -> anyhow::Result<Self> {
        Self::launch_and_init_with_log_dir(None).await
    }

    pub async fn launch_and_init_with_log_dir(log_dir: Option<&Path>) -> anyhow::Result<Self> {
        Self::launch_with_env(log_dir, &[])?.initialize().await
    }

    pub async fn launch_and_init_with_env(
        log_dir: Option<&Path>,
        envs: &[(&str, &str)],
    ) -> anyhow::Result<Self> {
        Self::launch_with_env(log_dir, envs)?.initialize().await
    }

    async fn initialize(mut self) -> anyhow::Result<Self> {
        let init = self
            .request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "synapse-test-utils",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;
        if init["serverInfo"]["name"] != "synapse-mcp" {
            bail!("unexpected initialize response: {init}");
        }
        self.notify("notifications/initialized", serde_json::json!({}))
            .await?;
        Ok(self)
    }

    pub fn launch(log_dir: Option<&Path>) -> anyhow::Result<Self> {
        Self::launch_with_env(log_dir, &[])
    }

    pub fn launch_with_env(log_dir: Option<&Path>, envs: &[(&str, &str)]) -> anyhow::Result<Self> {
        Self::launch_configured(log_dir, envs, |_command| {})
    }

    /// Like [`Self::launch_and_init_with_env`], with extra daemon CLI `args`
    /// after `--mode stdio` (e.g. `--allow-shell <pattern>`).
    pub async fn launch_and_init_with_args(
        log_dir: Option<&Path>,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> anyhow::Result<Self> {
        Self::launch_configured(log_dir, envs, |command| {
            command.args(args);
        })?
        .initialize()
        .await
    }

    /// Launches the daemon as the leader of a new console process group
    /// (`CREATE_NEW_PROCESS_GROUP`) so a test can deliver a real
    /// `CTRL_BREAK_EVENT` to it with `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)`
    /// without signalling the test process itself.
    #[cfg(windows)]
    pub async fn launch_and_init_in_new_process_group(
        log_dir: Option<&Path>,
    ) -> anyhow::Result<Self> {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        Self::launch_configured(log_dir, &[], |command| {
            command.creation_flags(CREATE_NEW_PROCESS_GROUP);
        })?
        .initialize()
        .await
    }

    fn launch_configured(
        log_dir: Option<&Path>,
        envs: &[(&str, &str)],
        configure: impl FnOnce(&mut Command),
    ) -> anyhow::Result<Self> {
        // One scratch home per client: the DB, logs, APPDATA and LOCALAPPDATA all
        // default under it so no test reads or writes the operator's real
        // %LOCALAPPDATA%\synapse tree, bridge token or log directory.
        let scratch_dir = tempfile::Builder::new()
            .prefix("synapse-stdio-home-")
            .tempdir()?;
        let mut command = Command::new(mcp_binary_path()?);
        command
            .args(["--mode", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_isolated_env(&mut command, scratch_dir.path(), log_dir, envs)?;
        configure(&mut command);

        let mut child = command.spawn().context("spawn synapse-mcp")?;
        let stdin = child.stdin.take().context("child stdin missing")?;
        let stdout = child.stdout.take().context("child stdout missing")?;
        let stderr = child.stderr.take().context("child stderr missing")?;
        let stderr_buffer = Arc::new(Mutex::new(Vec::new()));
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: BufReader::new(stdout).lines(),
            stderr_task: Some(tokio::spawn(read_stderr(
                stderr,
                Arc::clone(&stderr_buffer),
            ))),
            stderr_buffer,
            _scratch_dir: scratch_dir,
            catalog: None,
            next_id: 0,
            raw_rx: Vec::new(),
            raw_tx: Vec::new(),
        })
    }

    /// Tail of the daemon's stderr captured so far — the only diagnostics
    /// available when the child dies before answering a request.
    #[must_use]
    pub fn stderr_tail(&self, max_bytes: usize) -> String {
        let buffer = self
            .stderr_buffer
            .lock()
            .map(|buffer| buffer.clone())
            .unwrap_or_default();
        let start = buffer.len().saturating_sub(max_bytes);
        String::from_utf8_lossy(&buffer[start..]).into_owned()
    }

    /// `tools/list`, checked the way a strict MCP client checks it: a listing a
    /// strict client would reject (#548/#549) is an error here too. The
    /// listing becomes the catalog every later `tools_call` validates against.
    pub async fn tools_list(&mut self) -> anyhow::Result<Value> {
        let result = self.request("tools/list", serde_json::json!({})).await?;
        self.catalog = Some(ToolCatalog::from_tools_list(&result)?);
        Ok(result)
    }

    /// Validates `args` against the tool's listed `inputSchema` (refreshing the
    /// listing once if the tool is not cached yet), then sends the call.
    async fn send_validated_call(&mut self, name: &str, args: Value) -> anyhow::Result<Value> {
        if !self
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.contains(name))
        {
            self.tools_list().await?;
        }
        let catalog = self.catalog.as_ref().context("tool catalog missing")?;
        catalog.check_arguments(name, &args)?;
        self.request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": args,
            }),
        )
        .await
    }

    /// Production-client-parity call: arguments are validated against the
    /// listed `inputSchema` before send (`CLIENT_SCHEMA_INVALID`), a result with
    /// `isError: true` is a failure, and `structuredContent` must satisfy the
    /// tool's `outputSchema`. Use [`Self::tools_call_expect_tool_error`] when
    /// the test expects an `isError` result, and [`Self::tools_call_error`]
    /// when it expects a JSON-RPC error.
    pub async fn tools_call(&mut self, name: &str, args: Value) -> anyhow::Result<Value> {
        let result = self.send_validated_call(name, args).await?;
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
    /// failure (`isError: true`). Any other outcome is an error.
    pub async fn tools_call_expect_tool_error(
        &mut self,
        name: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        let result = self.send_validated_call(name, args).await?;
        if !is_tool_error(&result) {
            bail!("tools/call `{name}` was expected to return isError=true, got: {result}");
        }
        Ok(result)
    }

    /// Requests sent so far, one serialized JSON-RPC message per entry.
    #[must_use]
    pub fn raw_sent(&self) -> &[String] {
        &self.raw_tx
    }

    /// Server-side refusal probe: sends the call WITHOUT client-side schema
    /// validation (the test deliberately sends what a strict client might
    /// refuse, to prove the daemon refuses it too) and returns the JSON-RPC
    /// `error` object. A success response is an error.
    pub async fn tools_call_error(&mut self, name: &str, args: Value) -> anyhow::Result<Value> {
        self.request_error(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": args,
            }),
        )
        .await
    }

    pub async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&request).await?;

        let response = tokio::time::timeout(REQUEST_TIMEOUT, self.read_response(id))
            .await
            .with_context(|| {
                format!("timed out waiting for JSON-RPC response to {method} id {id}")
            })??;
        if let Some(error) = response.get("error") {
            bail!("JSON-RPC error from {method}: {error}");
        }
        response
            .get("result")
            .cloned()
            .with_context(|| format!("JSON-RPC response missing result: {response}"))
    }

    pub async fn request_error(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&request).await?;

        let response = tokio::time::timeout(REQUEST_TIMEOUT, self.read_response(id))
            .await
            .with_context(|| {
                format!("timed out waiting for JSON-RPC error response to {method} id {id}")
            })??;
        response
            .get("error")
            .cloned()
            .with_context(|| format!("JSON-RPC response missing error: {response}"))
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&notification).await
    }

    pub async fn shutdown(mut self) -> anyhow::Result<std::process::ExitStatus> {
        drop(self.stdin.take());
        let mut child = self.child.take().context("child already reaped")?;
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .context("timed out waiting for child shutdown")?
            .context("wait for child")?;

        if let Some(stderr_task) = self.stderr_task.take() {
            let _stderr = stderr_task.await.context("join stderr reader")?;
        }
        Ok(status)
    }

    /// Waits for the daemon to exit on its own (e.g. after a console control
    /// event) while stdin stays OPEN, so the exit cannot be the stdin-EOF
    /// path. Stdin is closed only after the child has exited.
    pub async fn wait_for_exit(
        mut self,
        timeout: Duration,
    ) -> anyhow::Result<std::process::ExitStatus> {
        let mut child = self.child.take().context("child already reaped")?;
        let status = tokio::time::timeout(timeout, child.wait())
            .await
            .context("timed out waiting for child to exit with stdin still open")?
            .context("wait for child")?;
        drop(self.stdin.take());
        if let Some(stderr_task) = self.stderr_task.take() {
            let _stderr = stderr_task.await.context("join stderr reader")?;
        }
        Ok(status)
    }

    #[must_use]
    pub fn raw_received(&self) -> &[String] {
        &self.raw_rx
    }

    #[must_use]
    pub fn child_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    async fn write_message(&mut self, value: &Value) -> anyhow::Result<()> {
        let line = serde_json::to_string(value)?;
        self.raw_tx.push(line.clone());
        let stdin = self.stdin.as_mut().context("child stdin closed")?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn read_response(&mut self, id: u64) -> anyhow::Result<Value> {
        while let Some(line) = self.stdout.next_line().await? {
            self.raw_rx.push(line.clone());
            let value: Value = serde_json::from_str(&line)
                .with_context(|| format!("parse JSON-RPC line: {line}"))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
        // Give the stderr reader a beat to drain the pipe before reporting.
        tokio::time::sleep(Duration::from_millis(100)).await;
        bail!(
            "MCP_TRANSPORT_CLOSED before response id {id}; daemon stderr tail:\n{}",
            self.stderr_tail(8 * 1024)
        );
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
            for _ in 0..20 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

async fn read_stderr(mut stderr: ChildStderr, shared: Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    let mut buf = [0_u8; 4096];
    loop {
        match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if let Ok(mut shared) = shared.lock() {
                    shared.extend_from_slice(&buf[..read]);
                }
            }
        }
    }
    shared
        .lock()
        .map(|shared| shared.clone())
        .unwrap_or_default()
}

/// Environment for every stdio test daemon: host `SYNAPSE_*` settings are
/// stripped, DB/logs/APPDATA/LOCALAPPDATA default under `scratch`, and the
/// caller's `envs` are applied last so they always win.
fn apply_isolated_env(
    command: &mut Command,
    scratch: &Path,
    log_dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> anyhow::Result<()> {
    let caller_sets = |name: &str| {
        envs.iter()
            .any(|(key, _value)| key.eq_ignore_ascii_case(name))
    };
    // Host SYNAPSE_* settings (SYNAPSE_ALLOW_*, SYNAPSE_RUN_SHELL_*, ...) must
    // not leak into assertions; every test states its own environment.
    for (key, _value) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|key| key.to_ascii_uppercase().starts_with("SYNAPSE_"))
        {
            command.env_remove(&key);
        }
    }
    command
        .env("SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY", "1")
        .env("SYNAPSE_LOG_LEVEL", "debug");
    // Default a deterministic synthetic foreground for every stdio MCP test.
    //
    // Action-gated tools (reflex_register, act_spawn_agent, task_dispatch_once,
    // act_*, ...) run the supported-use scope gate, which reads the current
    // foreground via current_audit_foreground() -> GetForegroundWindow(). With
    // no fixture that calls the REAL desktop and hard-fails A11Y_NO_FOREGROUND
    // whenever the host has no focused window (locked screen, focus elsewhere,
    // unattended run). Because the live foreground fluctuates between and during
    // runs, this made `cargo test --workspace` intermittently red in a way that
    // could not be pinned to one test. Injecting the synthetic notepad fixture
    // here makes the foreground a controlled input for the whole suite — one
    // place, no per-test drift. Tests that deliberately exercise a different
    // perception state (their own fixture, or force-no-perception /
    // force-observe-internal) set one of these keys and keep full control; the
    // value lookup in current_audit_foreground checks the force flags first.
    let caller_controls_perception = [
        "SYNAPSE_MCP_SYNTHETIC_FIXTURE",
        "SYNAPSE_MCP_FORCE_NO_PERCEPTION",
        "SYNAPSE_MCP_FORCE_OBSERVE_INTERNAL",
        "SYNAPSE_MCP_FORCE_NO_FOREGROUND",
    ]
    .into_iter()
    .any(caller_sets);
    if !caller_controls_perception {
        command.env("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad");
    }
    // #258: foreground routes refuse while a human typed recently, and a test
    // daemon has no input recorder (fail closed). Unrelated tests must not
    // depend on the host's human activity; tests of the gate set this key.
    if !caller_sets("SYNAPSE_USER_ACTIVE_QUIET_MS") {
        command.env("SYNAPSE_USER_ACTIVE_QUIET_MS", "0");
    }
    if !caller_sets("SYNAPSE_DB") {
        command.env("SYNAPSE_DB", scratch.join("db"));
    }
    for (name, sub) in [("APPDATA", "appdata"), ("LOCALAPPDATA", "localappdata")] {
        if !caller_sets(name) {
            let dir = scratch.join(sub);
            std::fs::create_dir_all(&dir)?;
            command.env(name, dir);
        }
    }
    match log_dir {
        Some(log_dir) => {
            command.env("SYNAPSE_LOG_DIR", log_dir);
        }
        None if !caller_sets("SYNAPSE_LOG_DIR") => {
            command.env("SYNAPSE_LOG_DIR", scratch.join("logs"));
        }
        None => {}
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    Ok(())
}

fn mcp_binary_path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("SYNAPSE_MCP_BIN") {
        return Ok(PathBuf::from(path));
    }
    std::env::var_os("CARGO_BIN_EXE_synapse-mcp")
        .map(PathBuf::from)
        .context("CARGO_BIN_EXE_synapse-mcp is unset; run from synapse-mcp integration tests or set SYNAPSE_MCP_BIN")
}
