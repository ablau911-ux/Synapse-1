//! #11: a daemon upgrade must not turn into a terminal `Transport closed` for
//! an already-running `--mode connect` bridge. These tests drive the REAL
//! bridge over stdio while the daemon behind it is replaced the way setup does
//! it (old daemon stopped, new daemon started on the same bind + DB), and
//! prove the SAME stdio MCP session answers `health` from the new daemon.
//! Source of truth for "which daemon": `<db>/daemon.pid` (single-instance lock).
//!
//! Every child is tokio `kill_on_drop(true)`; a daemon the bridge itself may
//! have detached is reaped by `DbDaemonReaper` on every exit path (#18).

#![cfg(windows)]

use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, bail};
use serde_json::{Value, json};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::Instant,
};

struct Home {
    dir: tempfile::TempDir,
    bind: String,
}

impl Home {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("synapse-g1b-11-")
            .tempdir()?;
        std::fs::create_dir_all(dir.path().join("appdata").join("synapse"))?;
        std::fs::create_dir_all(dir.path().join("local").join("synapse"))?;
        std::fs::write(dir.path().join("appdata/synapse/token.txt"), "g1b-11-token")?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let bind = listener.local_addr()?.to_string();
        drop(listener);
        Ok(Self { dir, bind })
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("db")
    }

    fn command(&self, mode: &str) -> Command {
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
            .args(["--mode", mode, "--bind", &self.bind, "--db"])
            .arg(self.db())
            .env("APPDATA", self.dir.path().join("appdata"))
            .env("LOCALAPPDATA", self.dir.path().join("local"))
            .env("SYNAPSE_LOG_DIR", self.dir.path().join("logs"))
            .env("SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY", "1")
            .kill_on_drop(true);
        command
    }

    fn start_daemon(&self) -> anyhow::Result<Child> {
        let mut command = self.command("http");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.spawn().context("spawn http daemon")
    }

    fn daemon_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.db().join("daemon.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn maintenance_lock(&self) -> PathBuf {
        self.dir
            .path()
            .join("local/synapse/setup-maintenance.lock.json")
    }
}

/// Kills whatever daemon `<db>/daemon.pid` names when the test ends, so a
/// detached daemon the bridge spawned can never outlive the test.
struct DbDaemonReaper(PathBuf);

impl Drop for DbDaemonReaper {
    fn drop(&mut self) {
        let Some(pid) = std::fs::read_to_string(self.0.join("daemon.pid"))
            .ok()
            .and_then(|raw| raw.trim().parse::<u32>().ok())
        else {
            return;
        };
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
        if let Some(process) = system.process(Pid::from_u32(pid))
            && process
                .exe()
                .is_some_and(|exe| exe.ends_with("synapse-mcp.exe"))
        {
            let _ = process.kill();
        }
    }
}

async fn wait_for_daemon_pid(home: &Home, pid: u32) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while home.daemon_pid() != Some(pid) {
        if Instant::now() > deadline {
            bail!(
                "daemon {pid} never owned the DB lock (pid file: {:?})",
                home.daemon_pid()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Minimal newline-delimited JSON-RPC client over the bridge's stdio.
struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Bridge {
    async fn start(home: &Home) -> anyhow::Result<Self> {
        let mut command = home.command("connect");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().context("spawn connect bridge")?;
        let stdin = child.stdin.take().context("bridge stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("bridge stdout")?).lines();
        let mut bridge = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        let init = bridge
            .request(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "g1b-11-bridge", "version": "1"}}),
            )
            .await?;
        anyhow::ensure!(init.get("result").is_some(), "initialize failed: {init}");
        bridge
            .send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        Ok(bridge)
    }

    async fn send(&mut self, message: &Value) -> anyhow::Result<()> {
        self.stdin
            .write_all(format!("{message}\n").as_bytes())
            .await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Full JSON-RPC response (result or error) for one request.
    async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        let read = async {
            while let Some(line) = self.stdout.next_line().await? {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && value["id"] == id
                {
                    return Ok(value);
                }
            }
            bail!("bridge stdout closed (terminal Transport closed) before response {id}")
        };
        tokio::time::timeout(Duration::from_secs(15), read).await?
    }

    /// `Ok(false)` = answered with an error or not yet (timeout, retry later);
    /// `Err` = the bridge's stdio closed, i.e. the terminal failure of #11.
    async fn health_ok(&mut self) -> anyhow::Result<bool> {
        let response = match self
            .request("tools/call", json!({"name": "health", "arguments": {}}))
            .await
        {
            Ok(response) => response,
            Err(error)
                if error
                    .downcast_ref::<tokio::time::error::Elapsed>()
                    .is_some() =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        Ok(response.get("result").is_some() && response["result"]["isError"] != true)
    }
}

#[tokio::test]
async fn must_pass_same_bridge_session_answers_from_replacement_daemon() -> anyhow::Result<()> {
    let home = Home::new()?;
    let _reaper = DbDaemonReaper(home.db());
    let mut old = home.start_daemon()?;
    let old_pid = old.id().context("old daemon pid")?;
    wait_for_daemon_pid(&home, old_pid).await?;
    let mut bridge = Bridge::start(&home).await?;
    assert!(
        bridge.health_ok().await?,
        "precondition: health via old daemon"
    );

    old.start_kill()?;
    old.wait().await?;
    let mut new = home.start_daemon()?;
    let new_pid = new.id().context("new daemon pid")?;
    wait_for_daemon_pid(&home, new_pid).await?;

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut healthy = false;
    while !healthy && Instant::now() < deadline {
        healthy = bridge.health_ok().await?;
        if !healthy {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    assert!(
        healthy,
        "same stdio session never answered health from the new daemon"
    );
    assert!(
        bridge.child.try_wait()?.is_none(),
        "bridge exited (terminal stdio EOF)"
    );
    assert_eq!(
        home.daemon_pid(),
        Some(new_pid),
        "health came from the replacement"
    );
    new.start_kill()?;
    Ok(())
}

#[tokio::test]
async fn must_fail_bridge_cold_start_spawns_no_daemon_while_setup_lock_is_held()
-> anyhow::Result<()> {
    let home = Home::new()?;
    let _reaper = DbDaemonReaper(home.db());
    let me = std::process::id();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(me)]), true);
    let start = system
        .process(Pid::from_u32(me))
        .context("own process")?
        .start_time();
    let lock = json!({"schema": "synapse_setup_maintenance_lock/v1", "state": "held",
        "pid": me, "pid_start_unix": start});
    std::fs::write(home.maintenance_lock(), lock.to_string())?;
    assert!(
        home.maintenance_lock().is_file(),
        "precondition: lock file written"
    );

    let mut command = home.command("connect");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let _bridge = command.spawn().context("spawn connect bridge")?;

    // Observation window: while setup holds the lock nothing may own the DB or the port.
    let window_end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < window_end {
        assert_eq!(
            home.daemon_pid(),
            None,
            "bridge spawned a daemon during setup maintenance"
        );
        assert!(
            std::net::TcpStream::connect(&home.bind).is_err(),
            "a daemon is listening on {} during setup maintenance",
            home.bind
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(())
}
