//! `--mode connect`: native stdio<->HTTP bridge to the shared Synapse daemon.
//!
//! Lets a stdio-only MCP client (Claude Desktop, Codex) reach the single shared
//! HTTP daemon instead of spawning its own embedded server (which would contend
//! for the one RocksDB lock). The bridge is a transport-level pump: it forwards
//! raw JSON-RPC between the client's stdio transport and an rmcp
//! Streamable-HTTP client transport pointed at the daemon, so the initialize
//! handshake, `Mcp-Session-Id` sessions, and SSE server->client notifications
//! are all handled by rmcp's client worker. No message interpretation, no
//! external proxy dependency.

use std::{path::Path, process::ExitCode, time::Duration};

use anyhow::Context;
use rmcp::{
    model::ClientJsonRpcMessage,
    transport::{
        Transport,
        async_rw::AsyncRwTransport,
        streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        },
    },
};
use tokio_util::sync::CancellationToken;

use crate::stdio_eof::CancelOnEofRead;

mod probe;
use probe::{
    DaemonProbe, HEALTH_PROBE_TIMEOUT, ProbeAction, auth_rejected, plan_for_probe, probe_daemon,
    probe_daemon_refreshing,
};

/// How long to wait for a freshly spawned daemon to become healthy.
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(15);
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Total time a bridge keeps trying to reach a daemon after losing it (#11).
/// Covers a setup-driven daemon replacement; past this the bridge fails closed.
const RECONNECT_WINDOW: Duration = Duration::from_mins(10);
/// During this initial part of the window the bridge only waits for an
/// externally (re)started daemon, so it never races setup's own start (#11).
const RECONNECT_SPAWN_GRACE: Duration = Duration::from_mins(1);
const RECONNECT_RETRY_DELAY: Duration = Duration::from_secs(1);
/// Canonical installed image name; setup retires a held image as
/// `synapse-mcp.exe.retired-*` (#11).
const CANONICAL_EXE_NAME: &str = "synapse-mcp.exe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconnectStep {
    WaitForDaemon,
    SpawnIfAbsent,
    GiveUp,
}

fn reconnect_step(elapsed: Duration) -> ReconnectStep {
    if elapsed >= RECONNECT_WINDOW {
        ReconnectStep::GiveUp
    } else if elapsed >= RECONNECT_SPAWN_GRACE {
        ReconnectStep::SpawnIfAbsent
    } else {
        ReconnectStep::WaitForDaemon
    }
}

const SETUP_MAINTENANCE_LOCK_SCHEMA: &str = "synapse_setup_maintenance_lock/v1";
/// Clock-resolution slack between setup's PowerShell `StartTime` and sysinfo's.
const OWNER_START_TOLERANCE_SECS: u64 = 2;
/// How long the bridge waits for setup to leave its lock-acquire critical
/// section before giving up on this spawn attempt.
#[cfg(windows)]
const SPAWN_GUARD_TIMEOUT_MS: u32 = 10_000;

/// True when the setup maintenance lock is `held` by a live setup process, so
/// the bridge must keep waiting for setup's daemon instead of spawning (#11).
/// Missing, unreadable, released, or dead-owner locks are stale and ignored.
/// `owner_start` maps a pid to its process start time (unix seconds), `None`
/// when dead; a live pid whose start time differs from the one setup recorded
/// is a reused PID, hence a stale owner.
fn setup_maintenance_held(
    contents: Option<&str>,
    owner_start: impl Fn(u32) -> Option<u64>,
) -> bool {
    let Some(contents) = contents else {
        return false;
    };
    let Ok(lock) =
        serde_json::from_str::<serde_json::Value>(contents.trim_start_matches('\u{feff}'))
    else {
        return false;
    };
    let schema_ok =
        lock.get("schema").and_then(|v| v.as_str()) == Some(SETUP_MAINTENANCE_LOCK_SCHEMA);
    let held = lock.get("state").and_then(|v| v.as_str()) == Some("held");
    let pid = lock
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok());
    let recorded_start = lock
        .get("pid_start_unix")
        .and_then(serde_json::Value::as_u64);
    let Some(pid) = pid.filter(|_| schema_ok && held) else {
        return false;
    };
    match (owner_start(pid), recorded_start) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(actual), Some(recorded)) => actual.abs_diff(recorded) <= OWNER_START_TOLERANCE_SECS,
    }
}

fn read_setup_maintenance_lock() -> Option<String> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let path = Path::new(&local)
        .join("synapse")
        .join("setup-maintenance.lock.json");
    std::fs::read_to_string(path).ok()
}

/// Start time (unix seconds) of a live process, `None` when it is not running.
fn process_start_unix(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let target = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
    system.process(target).map(sysinfo::Process::start_time)
}

/// Cross-process guard shared with `synapse-setup.ps1`
/// (`Enter-SynapseSetupSpawnGuard`): setup holds it while it acquires its
/// maintenance lock, the bridge holds it across lock-check + spawn, so the two
/// can never interleave (#11). Must match `$script:SynapseSetupSpawnGuardName`.
#[cfg(windows)]
struct SpawnGuard(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl SpawnGuard {
    fn acquire() -> anyhow::Result<Self> {
        use windows::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        use windows::core::w;

        // SAFETY: null security attributes; the name is a static NUL-terminated string.
        let handle =
            unsafe { CreateMutexW(None, false, w!("Global\\SynapseSetupMaintenanceSpawnGuard")) }
                .context("open setup spawn guard mutex")?;
        // SAFETY: `handle` is a valid mutex handle owned by this function.
        let wait = unsafe { WaitForSingleObject(handle, SPAWN_GUARD_TIMEOUT_MS) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            Ok(Self(handle))
        } else {
            // SAFETY: closing the handle we just opened.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
            anyhow::bail!(
                "MCP_CONNECT_SPAWN_GUARD_TIMEOUT: setup held the spawn guard for over {SPAWN_GUARD_TIMEOUT_MS}ms"
            )
        }
    }
}

#[cfg(windows)]
impl Drop for SpawnGuard {
    fn drop(&mut self) {
        // SAFETY: the handle is a mutex this thread owns (acquired in `acquire`).
        unsafe {
            let _ = windows::Win32::System::Threading::ReleaseMutex(self.0);
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
struct SpawnGuard;

#[cfg(not(windows))]
impl SpawnGuard {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "signature parity with the Windows guard"
    )]
    fn acquire() -> anyhow::Result<Self> {
        Ok(Self)
    }
}

/// Single choke point for every daemon self-spawn (initial startup and
/// reconnect). Takes the spawn guard first, then reads the maintenance lock and
/// spawns while still holding it, so setup cannot take its lock between the
/// check and the spawn. While a live setup holds the lock, do not spawn.
/// Returns whether `spawn` ran (#11).
fn spawn_unless_setup_maintenance<G>(
    acquire_guard: impl FnOnce() -> anyhow::Result<G>,
    read_lock: impl FnOnce() -> Option<String>,
    owner_start: impl Fn(u32) -> Option<u64>,
    spawn: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    let _guard = acquire_guard()?;
    if setup_maintenance_held(read_lock().as_deref(), owner_start) {
        return Ok(false);
    }
    spawn()?;
    Ok(true)
}

/// Executable used to self-spawn a daemon. A bridge whose image setup renamed
/// aside must spawn the freshly installed canonical binary, not its stale copy.
fn daemon_spawn_executable(current: &Path) -> std::path::PathBuf {
    let retired_prefix = format!("{CANONICAL_EXE_NAME}.retired-");
    let is_retired = current
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with(&retired_prefix));
    match (is_retired, current.parent()) {
        (true, Some(dir)) => dir.join(CANONICAL_EXE_NAME),
        _ => current.to_path_buf(),
    }
}

/// Arm a watchdog that exits the bridge if its parent (the MCP client) dies.
///
/// stdin EOF is the normal shutdown path, but on Windows an abrupt parent death
/// does not always deliver EOF to an inherited stdin (the original orphan
/// failure mode). This watchdog waits on the parent process handle and force
/// exits when it dies, so a bridge can never outlive its client. The shared
/// daemon is intentionally NOT subject to this — it must survive client churn.
fn install_parent_watchdog() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let Some(parent) = parent_process_info() else {
            tracing::error!(
                code = "MCP_CONNECT_PARENT_UNKNOWN",
                "could not determine parent process; refusing bridge without lifecycle owner"
            );
            anyhow::bail!(
                "MCP_CONNECT_PARENT_UNKNOWN: could not determine parent process; refusing bridge without lifecycle owner"
            );
        };
        if parent.is_unsupported_wsl_interop_host() {
            tracing::error!(
                code = "MCP_CONNECT_UNSUPPORTED_PARENT",
                parent_pid = parent.pid,
                parent_name = %parent.name,
                parent_command_line_len = parent.command_line.len(),
                parent_command_line_mentions_wsl = parent.command_line.to_ascii_lowercase().contains("\\wsl.exe"),
                remediation = "configure WSL clients for HTTP MCP transport or launch through a supported wrapper; direct WSL interop cannot prove client lifetime",
                "refusing direct WSL interop bridge parent"
            );
            anyhow::bail!(
                "MCP_CONNECT_UNSUPPORTED_PARENT: parent_pid={} parent_name={} direct WSL interop cannot prove client lifetime; configure WSL clients for HTTP MCP transport or a supported launcher",
                parent.pid,
                parent.name
            );
        }
        verify_parent_watchdog_handle(parent.pid)?;
        let parent_pid = parent.pid;
        std::thread::spawn(move || {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{
                OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
            };
            // SAFETY: OpenProcess with SYNCHRONIZE, then block on the handle.
            unsafe {
                match OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) {
                    Ok(handle) => {
                        WaitForSingleObject(handle, u32::MAX);
                        let _ = CloseHandle(handle);
                    }
                    Err(error) => {
                        tracing::error!(
                            code = "MCP_CONNECT_PARENT_OPEN_FAILED",
                            parent_pid,
                            error = %error,
                            "could not open parent process inside watchdog; bridge shutting down"
                        );
                        std::process::exit(1);
                    }
                }
            }
            tracing::warn!(
                code = "MCP_CONNECT_PARENT_EXITED",
                parent_pid,
                "parent client process exited; bridge shutting down"
            );
            std::process::exit(0);
        });
        tracing::info!(
            code = "MCP_CONNECT_PARENT_WATCHDOG",
            parent_pid,
            "parent-death watchdog armed"
        );
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Debug)]
struct ParentProcessInfo {
    pid: u32,
    name: String,
    command_line: String,
}

#[cfg(windows)]
impl ParentProcessInfo {
    fn is_unsupported_wsl_interop_host(&self) -> bool {
        self.name.eq_ignore_ascii_case("wsl.exe")
            || self.name.eq_ignore_ascii_case("wslhost.exe")
            || self.command_line.to_ascii_lowercase().contains("\\wsl.exe")
    }
}

#[cfg(windows)]
fn parent_process_info() -> Option<ParentProcessInfo> {
    use sysinfo::{ProcessesToUpdate, System, get_current_pid};
    let current = get_current_pid().ok()?;
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let parent_pid = system.process(current)?.parent()?;
    let parent = system.process(parent_pid)?;
    Some(ParentProcessInfo {
        pid: parent_pid.as_u32(),
        name: parent.name().to_string_lossy().into_owned(),
        command_line: parent
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
    })
}

#[cfg(windows)]
fn verify_parent_watchdog_handle(parent_pid: u32) -> anyhow::Result<()> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};

    // SAFETY: OpenProcess is called with synchronize-only access. The handle is
    // closed immediately; the watchdog thread opens its own handle after spawn.
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                Ok(())
            }
            Err(error) => {
                tracing::error!(
                    code = "MCP_CONNECT_PARENT_OPEN_FAILED",
                    parent_pid,
                    error = %error,
                    "could not open parent process; refusing bridge without watchdog"
                );
                anyhow::bail!(
                    "MCP_CONNECT_PARENT_OPEN_FAILED: could not open parent pid {parent_pid}: {error}"
                );
            }
        }
    }
}

/// Spawn the shared daemon detached (its own stdio = null so it never writes to
/// the bridge's MCP stdout, and it outlives the bridge). The T1 single-instance
/// guard ensures that if several bridges race to spawn, only one daemon wins.
#[cfg(not(windows))]
fn spawn_detached_daemon(bind: &str, db: Option<&Path>) -> anyhow::Result<()> {
    let exe = daemon_spawn_executable(
        &std::env::current_exe().context("resolve current executable path")?,
    );
    crate::m4::child_environment::TrustedBootstrap::SharedDaemon.keep_inherited_environment();
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--mode", "http", "--bind", bind]);
    if let Some(db) = db {
        cmd.arg("--db").arg(db);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().context("spawn shared daemon process")?;
    Ok(())
}

/// Spawn the daemon on Windows with `bInheritHandles = FALSE` via
/// `CreateProcessW`. This is critical: `std::process::Command` spawns with
/// handle inheritance enabled, which would leak the stdio pipe handles
/// connecting an MCP client to this bridge into the long-lived daemon — keeping
/// those pipes open so the client could never detect the bridge exiting. With
/// inheritance disabled the detached daemon shares none of our handles.
#[cfg(windows)]
fn spawn_detached_daemon(bind: &str, db: Option<&Path>) -> anyhow::Result<()> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    };
    use windows::core::{PCWSTR, PWSTR};

    crate::m4::child_environment::TrustedBootstrap::SharedDaemon.keep_inherited_environment();
    let exe = daemon_spawn_executable(
        &std::env::current_exe().context("resolve current executable path")?,
    );
    let mut command_line = String::new();
    command_line.push('"');
    command_line.push_str(&exe.to_string_lossy());
    command_line.push_str("\" --mode http --bind ");
    command_line.push_str(bind);
    if let Some(db) = db {
        command_line.push_str(" --db \"");
        command_line.push_str(&db.to_string_lossy());
        command_line.push('"');
    }
    let mut command_line_w: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let startup_info = STARTUPINFOW {
        cb: u32::try_from(core::mem::size_of::<STARTUPINFOW>()).unwrap_or(0),
        ..Default::default()
    };
    let mut process_info = PROCESS_INFORMATION::default();

    // SAFETY: command_line_w is a writable, NUL-terminated UTF-16 buffer kept
    // alive across the call; all optional pointers are null; bInheritHandles is
    // false so the daemon inherits none of this process's handles.
    let result = unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(command_line_w.as_mut_ptr())),
            None,
            None,
            false,
            DETACHED_PROCESS | CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &startup_info,
            &mut process_info,
        )
    };
    result.context("CreateProcessW for shared daemon")?;

    // SAFETY: handles from a successful CreateProcessW; we do not need them.
    unsafe {
        let _ = CloseHandle(process_info.hProcess);
        let _ = CloseHandle(process_info.hThread);
    }
    Ok(())
}

/// Ensure a shared daemon is reachable at `bind` (#171). Spawns one (guarded)
/// only when the port refuses connections and no live daemon holds the
/// single-instance lock; a 401/403 is a terminal `MCP_CONNECT_AUTH_REJECTED`, and
/// a slow or non-2xx answer just waits. Errors (no fallback) if the daemon never
/// becomes healthy within [`DAEMON_READY_TIMEOUT`]. On a 401/403 the token is
/// re-read once; the returned token is the one the daemon accepted.
async fn ensure_daemon_running(
    bind: &str,
    db: Option<&Path>,
    token: &str,
) -> anyhow::Result<String> {
    let (probe, token) = probe_daemon_refreshing(bind, token, HEALTH_PROBE_TIMEOUT).await;
    let db_path = db.map_or_else(crate::m3::default_db_path, Path::to_path_buf);
    let spawned = match plan_for_probe(&probe, || {
        crate::single_instance::SingleInstanceGuard::is_held(&db_path)
    }) {
        ProbeAction::Proceed => {
            tracing::info!(
                code = "MCP_CONNECT_DAEMON_PRESENT",
                bind = %bind,
                "shared daemon already running"
            );
            return Ok(token);
        }
        ProbeAction::AuthRejected(status) => return Err(auth_rejected(bind, status)),
        ProbeAction::WaitForDaemon => {
            tracing::info!(
                code = "MCP_CONNECT_DAEMON_NOT_SPAWNING",
                bind = %bind,
                probe = ?probe,
                "daemon is listening, slow, or holds the single-instance lock; waiting instead of spawning"
            );
            false
        }
        ProbeAction::Spawn => {
            tracing::info!(
                code = "MCP_CONNECT_DAEMON_SPAWNING",
                bind = %bind,
                "port refused and no live daemon lock; spawning shared daemon"
            );
            let spawned = spawn_unless_setup_maintenance(
                SpawnGuard::acquire,
                read_setup_maintenance_lock,
                process_start_unix,
                || spawn_detached_daemon(bind, db),
            )
            .context("spawn shared daemon")?;
            if !spawned {
                tracing::info!(
                    code = "MCP_CONNECT_SETUP_MAINTENANCE_HELD",
                    bind = %bind,
                    "setup maintenance lock held; waiting for setup's daemon instead of spawning"
                );
            }
            spawned
        }
    };
    wait_until_healthy(bind, &token, spawned).await?;
    Ok(token)
}

/// Poll `/health` until the daemon is healthy or [`DAEMON_READY_TIMEOUT`] passes.
/// A 401/403 while waiting is terminal; timeouts and other answers keep waiting.
async fn wait_until_healthy(bind: &str, token: &str, spawned: bool) -> anyhow::Result<()> {
    wait_until_healthy_within(bind, token, spawned, DAEMON_READY_TIMEOUT).await
}

/// [`wait_until_healthy`] with an explicit wall-clock `budget`. The wait is a time
/// deadline (not a probe count), so slow or refused probes cannot stretch it.
async fn wait_until_healthy_within(
    bind: &str,
    token: &str,
    spawned: bool,
    budget: Duration,
) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + budget;
    let mut attempt = 0u32;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(DAEMON_POLL_INTERVAL.min(remaining)).await;
        attempt = attempt.saturating_add(1);
        // A probe never outlives the budget: cap its timeout by the time left.
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let probe_timeout = HEALTH_PROBE_TIMEOUT.min(remaining.max(Duration::from_millis(50)));
        match probe_daemon(bind, token, probe_timeout).await {
            DaemonProbe::Healthy => {
                tracing::info!(
                    code = "MCP_CONNECT_DAEMON_READY",
                    bind = %bind,
                    attempts = attempt,
                    "daemon is healthy"
                );
                return Ok(());
            }
            DaemonProbe::AuthRejected(status) => return Err(auth_rejected(bind, status)),
            _ => {}
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
    }
    if spawned {
        anyhow::bail!(
            "MCP_DAEMON_SPAWN_FAILED: shared daemon at {bind} did not become healthy within {}s after spawn",
            budget.as_secs()
        );
    }
    anyhow::bail!(
        "MCP_CONNECT_DAEMON_UNRESPONSIVE: daemon at {bind} did not become healthy within {}s; not spawning a second daemon (the port is listening, a live daemon holds the single-instance lock, or setup is replacing it)",
        budget.as_secs()
    );
}

fn new_daemon_transport(uri: &str, token: &str) -> StreamableHttpClientTransport<reqwest::Client> {
    let config =
        StreamableHttpClientTransportConfig::with_uri(uri.to_owned()).auth_header(token.to_owned());
    StreamableHttpClientTransport::from_config(config)
}

async fn open_daemon_transport(
    bind: &str,
    uri: &str,
    db: Option<&Path>,
    token: &str,
) -> anyhow::Result<StreamableHttpClientTransport<reqwest::Client>> {
    let token = ensure_daemon_running(bind, db, token)
        .await
        .context("ensure shared daemon is running")?;
    Ok(new_daemon_transport(uri, &token))
}

async fn reconnect_daemon_transport(
    bind: &str,
    uri: &str,
    db: Option<&Path>,
    token: &str,
    saved_initialize: Option<&ClientJsonRpcMessage>,
    saved_initialized: Option<&ClientJsonRpcMessage>,
) -> anyhow::Result<StreamableHttpClientTransport<reqwest::Client>> {
    let Some(initialize_message) = saved_initialize.cloned() else {
        anyhow::bail!("MCP_CONNECT_RECONNECT_NO_INITIALIZE: cannot replay bridge handshake");
    };
    let Some(initialized_message) = saved_initialized.cloned() else {
        anyhow::bail!("MCP_CONNECT_RECONNECT_NO_INITIALIZED: cannot replay bridge handshake");
    };

    let mut daemon = open_daemon_transport(bind, uri, db, token).await?;
    daemon
        .send(initialize_message)
        .await
        .context("replay initialize to daemon after reconnect")?;
    let Some(_initialize_response) = tokio::time::timeout(DAEMON_READY_TIMEOUT, daemon.receive())
        .await
        .context("wait for replayed initialize response after daemon reconnect")?
    else {
        anyhow::bail!(
            "MCP_CONNECT_RECONNECT_INIT_EOF: daemon closed before replayed initialize response"
        );
    };
    daemon
        .send(initialized_message)
        .await
        .context("replay initialized notification to daemon after reconnect")?;
    tracing::info!(
        code = "MCP_CONNECT_DAEMON_RECONNECTED",
        bind = %bind,
        "reconnected daemon transport and replayed MCP handshake"
    );
    Ok(daemon)
}

/// Keep reconnecting across a daemon replacement instead of ending the bridge
/// on the first failed attempt (#11). Waits for an externally started daemon
/// during [`RECONNECT_SPAWN_GRACE`], then may self-spawn, and fails closed after
/// [`RECONNECT_WINDOW`].
async fn reconnect_daemon_transport_with_retry(
    bind: &str,
    uri: &str,
    db: Option<&Path>,
    token: &str,
    saved_initialize: Option<&ClientJsonRpcMessage>,
    saved_initialized: Option<&ClientJsonRpcMessage>,
) -> anyhow::Result<StreamableHttpClientTransport<reqwest::Client>> {
    let started = std::time::Instant::now();
    let mut attempts = 0u32;
    loop {
        match reconnect_step(started.elapsed()) {
            ReconnectStep::GiveUp => anyhow::bail!(
                "MCP_CONNECT_RECONNECT_WINDOW_EXHAUSTED: daemon at {bind} unreachable for {}s after {attempts} attempts; restart the MCP client",
                RECONNECT_WINDOW.as_secs()
            ),
            ReconnectStep::WaitForDaemon => {
                match probe_daemon_refreshing(bind, token, HEALTH_PROBE_TIMEOUT)
                    .await
                    .0
                {
                    DaemonProbe::Healthy => {}
                    // Revoked credential: waiting will not fix it (#171).
                    DaemonProbe::AuthRejected(status) => return Err(auth_rejected(bind, status)),
                    _ => {
                        tokio::time::sleep(RECONNECT_RETRY_DELAY).await;
                        continue;
                    }
                }
            }
            ReconnectStep::SpawnIfAbsent => {}
        }
        attempts = attempts.saturating_add(1);
        match reconnect_daemon_transport(bind, uri, db, token, saved_initialize, saved_initialized)
            .await
        {
            Ok(daemon) => return Ok(daemon),
            Err(error) => {
                if error.downcast_ref::<probe::AuthRejectedError>().is_some() {
                    return Err(error);
                }
                tracing::warn!(
                    code = "MCP_CONNECT_RECONNECT_RETRY",
                    bind = %bind,
                    attempts,
                    elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    error = %format!("{error:#}"),
                    "daemon reconnect attempt failed; retrying"
                );
                tokio::time::sleep(RECONNECT_RETRY_DELAY).await;
            }
        }
    }
}

/// Run the stdio<->HTTP bridge against the daemon listening at `bind`
/// (`host:port`). Exits 0 when the client closes stdin; daemon stream loss is
/// repaired by reopening the HTTP transport and replaying the MCP handshake.
pub async fn run_connect(bind: &str, db: Option<&Path>) -> anyhow::Result<ExitCode> {
    let uri = format!("http://{bind}/mcp");
    // #22: inside a spawned agent the scoped spawn credential wins over token.txt.
    let token = crate::spawn_credentials::client_bearer_token()
        .context("load daemon bearer token for bridge")?;
    tracing::info!(
        code = "MCP_CONNECT_STARTING",
        daemon_uri = %uri,
        "starting stdio<->http bridge to shared daemon"
    );

    // Arm the parent-death watchdog before anything else so the bridge can
    // never outlive the client that launched it.
    install_parent_watchdog()?;

    // Ensure exactly one shared daemon is up (spawn it if needed) before bridging.
    let mut daemon = open_daemon_transport(bind, &uri, db, &token).await?;

    let client_closed_token = CancellationToken::new();
    let (stdin, stdout) = rmcp::transport::stdio();
    let stdin = CancelOnEofRead::new(
        stdin,
        client_closed_token.clone(),
        client_closed_token.clone(),
        "MCP_CONNECT_EOF_CONNECTION_CLOSED",
        "connect",
    );
    let mut client = AsyncRwTransport::new_server(stdin, stdout);
    let mut client_message_count = 0usize;
    let mut saved_initialize: Option<ClientJsonRpcMessage> = None;
    let mut saved_initialized: Option<ClientJsonRpcMessage> = None;

    loop {
        tokio::select! {
            _ = client_closed_token.cancelled() => {
                tracing::info!(
                    code = "MCP_CONNECT_STDIN_EOF",
                    "client stdin EOF guard cancelled bridge; shutting down bridge"
                );
                break;
            }
            from_client = client.receive() => {
                match from_client {
                    Some(message) => {
                        let message_index = client_message_count;
                        client_message_count = client_message_count.saturating_add(1);
                        match message_index {
                            0 => saved_initialize = Some(message.clone()),
                            1 => saved_initialized = Some(message.clone()),
                            _ => {}
                        }

                        if let Err(error) = daemon.send(message.clone()).await {
                            tracing::warn!(
                                code = "MCP_CONNECT_CLIENT_SEND_FAILED",
                                error = %error,
                                "client->daemon send failed; attempting daemon reconnect"
                            );
                            if message_index == 0 {
                                daemon = open_daemon_transport(bind, &uri, db, &token).await?;
                                daemon
                                    .send(message)
                                    .await
                                    .context("forward initial client->daemon message after reconnect")?;
                            } else {
                                daemon = reconnect_daemon_transport_with_retry(
                                    bind,
                                    &uri,
                                    db,
                                    &token,
                                    saved_initialize.as_ref(),
                                    saved_initialized.as_ref(),
                                )
                                .await
                                .context("reconnect daemon after client->daemon send failure")?;
                                if message_index != 1 {
                                    daemon
                                        .send(message)
                                        .await
                                        .context("forward client->daemon message after reconnect")?;
                                }
                            }
                        }
                    }
                    None => {
                        tracing::info!(
                            code = "MCP_CONNECT_STDIN_EOF",
                            "client closed stdin; shutting down bridge"
                        );
                        break;
                    }
                }
            }
            from_daemon = daemon.receive() => {
                match from_daemon {
                    Some(message) => client
                        .send(message)
                        .await
                        .context("forward daemon->client message")?,
                    None => {
                        tracing::warn!(
                            code = "MCP_CONNECT_DAEMON_CLOSED",
                            "daemon stream closed; attempting reconnect"
                        );
                        daemon = reconnect_daemon_transport_with_retry(
                            bind,
                            &uri,
                            db,
                            &token,
                            saved_initialize.as_ref(),
                            saved_initialized.as_ref(),
                        )
                        .await
                        .context("reconnect daemon after stream close")?;
                    }
                }
            }
        }
    }

    // Bound shutdown: close() can block (e.g. HTTP session-delete, or a daemon
    // transport whose worker never initialized). Never let cleanup hang the
    // bridge — any lingering rmcp worker task is aborted when the runtime drops
    // on return.
    let _ = tokio::time::timeout(Duration::from_secs(3), daemon.close()).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), client.close()).await;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
#[path = "tests/connect_upgrade_reconnect_11.rs"]
mod connect_upgrade_reconnect_11;

#[cfg(test)]
#[path = "tests/connect_probe_171.rs"]
mod connect_probe_171;

#[cfg(test)]
#[path = "tests/connect_deadline_229.rs"]
mod connect_deadline_229;
