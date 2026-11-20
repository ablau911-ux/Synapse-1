//! Windows process-lifecycle coverage for the shared-daemon work. The legacy
//! `drop_kills_child.rs` / `sigint_clean_exit.rs` are `#![cfg(unix)]`, leaving
//! the exact platform where orphans accumulated untested. These exercise the
//! real binary (no mocks) on Windows: stdio servers exit on stdin EOF, and a
//! duplicate daemon is refused by the single-instance guard.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

// Same kill-and-reap guard used by session_failed_process_retain_75 tests.
// This integration binary cannot import that private unit-test module.
struct ChildGuard(std::process::Child);

impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn guard_probe_child_18() -> anyhow::Result<ChildGuard> {
    let child = Command::new("ping.exe")
        .args(["-n", "120", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(ChildGuard)?;
    Ok(child)
}

#[test]
fn must_pass_child_guard_reaps_its_exact_child_on_drop_18() -> anyhow::Result<()> {
    let mut child = guard_probe_child_18()?;
    let pid = child.id();
    assert_eq!(child.try_wait()?, None);
    assert!(process_alive(pid), "precondition: probe child exists");
    drop(child);
    assert!(!process_alive(pid), "guard must kill and reap its child");
    Ok(())
}

#[test]
fn must_fail_early_error_and_panic_leave_no_guarded_child_18() -> anyhow::Result<()> {
    for panic_exit in [false, true] {
        let mut child = guard_probe_child_18()?;
        let pid = child.id();
        assert_eq!(child.try_wait()?, None);
        assert!(process_alive(pid), "precondition: probe child exists");
        let result = std::panic::catch_unwind(move || -> anyhow::Result<()> {
            let _child = child;
            if panic_exit {
                panic!("synthetic assertion failure");
            }
            bail!("synthetic setup error");
        });
        if panic_exit {
            assert!(result.is_err(), "panic obstacle must occur");
        } else {
            assert_eq!(
                result.unwrap().unwrap_err().to_string(),
                "synthetic setup error"
            );
        }
        assert!(
            !process_alive(pid),
            "early exit must kill and reap its child"
        );
    }
    Ok(())
}

const fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_synapse-mcp")
}

fn db_arg(dir: &Path) -> anyhow::Result<String> {
    dir.join("db")
        .to_str()
        .context("temp db path is not valid UTF-8")
        .map(ToOwned::to_owned)
}

fn free_loopback_bind() -> anyhow::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind ephemeral loopback port")?;
    let port = listener
        .local_addr()
        .context("read ephemeral local addr")?
        .port();
    Ok(format!("127.0.0.1:{port}"))
}

fn write_test_token_appdata(root: &Path) -> anyhow::Result<PathBuf> {
    let appdata = root.join("appdata");
    fs::create_dir_all(appdata.join("synapse"))?;
    fs::write(appdata.join("synapse").join("token.txt"), "test-token")?;
    Ok(appdata)
}

fn process_alive(pid: u32) -> bool {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system.process(Pid::from_u32(pid)).is_some()
}

fn wait_for_pid_file(path: &Path, timeout: Duration) -> anyhow::Result<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(raw) = fs::read_to_string(path) {
            return raw
                .trim()
                .parse::<u32>()
                .with_context(|| format!("parse pid file {}", path.display()));
        }
        if Instant::now() > deadline {
            bail!("pid file did not appear: {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_pid_exit(pid: u32, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if !process_alive(pid) {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("pid {pid} did not exit within {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn kill_process(pid: u32) {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    if let Some(process) = system.process(Pid::from_u32(pid)) {
        let _ = process.kill();
    }
}

fn kill_synapse_processes_for_db(db: &str) {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );
    for process in system.processes().values() {
        if !process
            .name()
            .to_string_lossy()
            .to_lowercase()
            .contains("synapse-mcp")
        {
            continue;
        }
        let cmd = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        if cmd.contains(db) {
            let _ = process.kill();
        }
    }
}

fn windows_path_to_wsl(path: &str) -> anyhow::Result<String> {
    let mut chars = path.chars();
    let drive = chars
        .next()
        .context("Windows path missing drive letter")?
        .to_ascii_lowercase();
    if chars.next() != Some(':') {
        bail!("Windows path should start with drive colon: {path}");
    }
    let rest = chars.as_str().replace('\\', "/");
    Ok(format!("/mnt/{drive}{rest}"))
}

fn lifecycle_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn ps_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

fn bridge_logs(dir: &Path) -> anyhow::Result<String> {
    let mut logs = String::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            logs.push_str(&fs::read_to_string(entry.path())?);
        }
    }
    Ok(logs)
}

fn wait_for_parent_watchdog(dir: &Path, pid: u32) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let logs = bridge_logs(dir)?;
        if logs.contains("MCP_CONNECT_PARENT_WATCHDOG") {
            return Ok(());
        }
        anyhow::ensure!(
            process_alive(pid),
            "bridge exited before watchdog armed: {logs}"
        );
        anyhow::ensure!(Instant::now() < deadline, "watchdog did not arm: {logs}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The connect bridge must exit when its parent process dies, even if stdin EOF
/// is not delivered. The daemon it connects to is intentionally cleaned up by
/// DB path after the bridge exit assertion.
#[test]
fn must_fail_connect_bridge_exits_when_parent_process_dies() -> anyhow::Result<()> {
    let _guard = lifecycle_test_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = tempfile::tempdir()?;
    let db = db_arg(tmp.path())?;
    let bind = free_loopback_bind()?;
    let appdata = write_test_token_appdata(tmp.path())?;

    let bridge_pid_file = tmp.path().join("bridge.pid");
    let bridge_logs = tmp.path().join("bridge-logs");
    fs::create_dir_all(&bridge_logs)?;
    let parent_script = tmp.path().join("launch_bridge_parent.ps1");
    let script = format!(
        "$env:APPDATA = {}\n\
         $env:LOCALAPPDATA = {}\n\
         $env:SYNAPSE_LOG_DIR = {}\n\
         $p = Start-Process -FilePath {} -ArgumentList @('--mode','connect','--bind',{},'--db',{},'--log-level','info') -WindowStyle Hidden -PassThru\n\
         Set-Content -Path {} -Value $p.Id\n\
         Start-Sleep -Seconds 300\n",
        ps_quote(&appdata.display().to_string()),
        ps_quote(&tmp.path().display().to_string()),
        ps_quote(&bridge_logs.display().to_string()),
        ps_quote(bin()),
        ps_quote(&bind),
        ps_quote(&db),
        ps_quote(&bridge_pid_file.display().to_string()),
    );
    fs::write(&parent_script, script)?;

    let mut parent = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            parent_script
                .to_str()
                .context("parent script path is not valid UTF-8")?,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(ChildGuard)
        .context("spawn bridge parent process")?;

    let bridge_pid = match wait_for_pid_file(&bridge_pid_file, Duration::from_secs(20)) {
        Ok(pid) => pid,
        Err(error) => {
            let _ = parent.kill();
            let _ = parent.wait();
            kill_synapse_processes_for_db(&db);
            return Err(error);
        }
    };
    if !process_alive(bridge_pid) {
        let _ = parent.kill();
        let _ = parent.wait();
        kill_synapse_processes_for_db(&db);
        bail!("bridge pid {bridge_pid} exited before parent-death trigger");
    }

    // Process creation is not watchdog readiness: killing the parent before
    // discovery races parent_process_info and tests startup refusal instead.
    if let Err(error) = wait_for_parent_watchdog(&bridge_logs, bridge_pid) {
        let _ = parent.kill();
        let _ = parent.wait();
        kill_process(bridge_pid);
        kill_synapse_processes_for_db(&db);
        return Err(error);
    }
    parent.kill().context("kill bridge parent process")?;
    let _ = parent.wait();
    let exit_result = wait_for_pid_exit(bridge_pid, Duration::from_secs(20));
    if exit_result.is_err() {
        kill_process(bridge_pid);
    }
    kill_synapse_processes_for_db(&db);
    exit_result?;
    // The exit must be the parent-death watchdog, not a crash.
    let mut logs = String::new();
    for entry in fs::read_dir(&bridge_logs)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            logs.push_str(&fs::read_to_string(entry.path())?);
        }
    }
    assert!(
        logs.contains("MCP_CONNECT_PARENT_EXITED"),
        "bridge must log the parent-death shutdown: {logs}"
    );
    Ok(())
}

/// The connect bridge must also exit when stdin reaches EOF before the client
/// sends a complete MCP initialize frame. This is the empty-input boundary that
/// can otherwise leave a bridge waiting forever under a still-alive parent.
#[test]
fn must_fail_connect_bridge_exits_on_stdin_eof_before_init() -> anyhow::Result<()> {
    let _guard = lifecycle_test_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = tempfile::tempdir()?;
    let db = db_arg(tmp.path())?;
    let bind = free_loopback_bind()?;
    let appdata = write_test_token_appdata(tmp.path())?;
    let mut child = Command::new(bin())
        .args([
            "--mode",
            "connect",
            "--bind",
            &bind,
            "--db",
            &db,
            "--log-level",
            "info",
        ])
        .env("APPDATA", &appdata)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(ChildGuard)
        .context("spawn connect bridge")?;

    std::thread::sleep(Duration::from_millis(800));
    drop(child.stdin.take());

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().context("poll connect bridge")? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            kill_synapse_processes_for_db(&db);
            bail!("connect bridge did not exit within 20s after stdin EOF");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // The process has exited; retain guard ownership while reading its pipe.
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        std::io::Read::read_to_string(&mut pipe, &mut stderr)
            .context("collect connect bridge stderr")?;
    }
    kill_synapse_processes_for_db(&db);

    assert!(status.success(), "connect bridge exit status: {status}");
    assert!(
        stderr.contains("MCP_CONNECT_EOF_CONNECTION_CLOSED"),
        "stderr should contain EOF guard log; stderr={stderr}"
    );
    assert!(
        stderr.contains("MCP_CONNECT_STDIN_EOF"),
        "stderr should contain bridge EOF shutdown log; stderr={stderr}"
    );
    Ok(())
}

/// A second daemon for the same DB must be refused by the single-instance guard
/// (exit code 3) while the first keeps running.
#[test]
fn must_fail_duplicate_daemon_is_refused() -> anyhow::Result<()> {
    let _guard = lifecycle_test_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = tempfile::tempdir()?;
    let db = db_arg(tmp.path())?;
    let pid_file = tmp.path().join("db").join("daemon.pid");
    let appdata = write_test_token_appdata(tmp.path())?;
    let first_bind = free_loopback_bind()?;
    let second_bind = free_loopback_bind()?;
    let second_logs = tmp.path().join("second-logs");
    fs::create_dir_all(&second_logs)?;

    let mut first = Command::new(bin())
        .args([
            "--mode",
            "http",
            "--bind",
            &first_bind,
            "--db",
            &db,
            "--log-level",
            "warn",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("APPDATA", &appdata)
        .spawn()
        .map(ChildGuard)
        .context("spawn first daemon")?;

    // Wait until the first daemon has acquired the guard (records its PID).
    let deadline = Instant::now() + Duration::from_secs(20);
    while !pid_file.exists() {
        if let Some(status) = first.try_wait().context("poll first daemon startup")? {
            bail!("first daemon exited before acquiring lock: {status}");
        }
        if Instant::now() > deadline {
            let _ = first.kill();
            bail!("first daemon never acquired the single-instance lock");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Second daemon, same DB, different port -> guard refuses before bind.
    let second = Command::new(bin())
        .args([
            "--mode",
            "http",
            "--bind",
            &second_bind,
            "--db",
            &db,
            "--log-level",
            "warn",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("APPDATA", &appdata)
        .env("SYNAPSE_LOG_DIR", &second_logs)
        .output()
        .context("run second daemon")?;

    let first_still_alive = first.try_wait().context("poll first daemon")?.is_none();
    // Source of truth after the refusal: the lock record still names the first
    // daemon, and the second never bound its port.
    let recorded_pid = fs::read_to_string(&pid_file)
        .context("read daemon.pid after duplicate launch")?
        .trim()
        .to_owned();
    let second_port_free = std::net::TcpListener::bind(&second_bind).is_ok();
    let first_pid = first.id();
    let _ = first.kill();
    let _ = first.wait();

    let mut second_log_text = String::new();
    for entry in fs::read_dir(&second_logs)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            second_log_text.push_str(&fs::read_to_string(entry.path())?);
        }
    }
    assert!(
        second_log_text.contains("MCP_DAEMON_ALREADY_RUNNING"),
        "duplicate refusal must name the single-instance guard: {second_log_text}"
    );
    assert!(
        recorded_pid.contains(&first_pid.to_string()),
        "daemon.pid must still hold the first daemon {first_pid}: {recorded_pid:?}"
    );
    assert!(
        second_port_free,
        "refused duplicate must not bind {second_bind}"
    );

    assert_eq!(
        second.status.code(),
        Some(3),
        "duplicate daemon should exit 3 via the single-instance guard"
    );
    assert!(
        first_still_alive,
        "first daemon should survive the duplicate launch"
    );
    Ok(())
}

/// A stdio server whose stdin stays open must keep running (no premature exit),
/// and then exit cleanly once stdin reaches EOF.
#[test]
fn must_pass_stdio_server_stays_alive_while_stdin_open() -> anyhow::Result<()> {
    let _guard = lifecycle_test_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = tempfile::tempdir()?;
    let db = db_arg(tmp.path())?;
    let mut child = Command::new(bin())
        .args(["--mode", "stdio", "--db", &db, "--log-level", "warn"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map(ChildGuard)
        .context("spawn stdio server")?;

    std::thread::sleep(Duration::from_secs(3));
    if let Some(status) = child.try_wait().context("poll stdio server")? {
        bail!("stdio server exited while stdin was still open: {status}");
    }

    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().context("poll stdio server")? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("stdio server did not exit within 15s after stdin EOF");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "stdio server EOF exit status: {status}");
    Ok(())
}

/// The single-instance lock is released by the OS when its holder dies, so a
/// crashed (killed) daemon never wedges the next launch for the same DB.
#[test]
fn must_pass_daemon_relaunch_after_holder_killed_acquires_guard() -> anyhow::Result<()> {
    let _guard = lifecycle_test_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = tempfile::tempdir()?;
    let db = db_arg(tmp.path())?;
    let pid_file = tmp.path().join("db").join("daemon.pid");
    let appdata = write_test_token_appdata(tmp.path())?;

    let spawn_daemon = |bind: &str| {
        Command::new(bin())
            .args([
                "--mode",
                "http",
                "--bind",
                bind,
                "--db",
                &db,
                "--log-level",
                "warn",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("APPDATA", &appdata)
            .spawn()
            .map(ChildGuard)
    };
    let wait_for_holder = |child: &mut std::process::Child| -> anyhow::Result<()> {
        let expected = child.id();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let holder = fs::read_to_string(&pid_file)
                .ok()
                .and_then(|raw| raw.trim().parse::<u32>().ok());
            if holder == Some(expected) {
                return Ok(());
            }
            if let Some(status) = child.try_wait().context("poll daemon startup")? {
                bail!("daemon {expected} exited before acquiring lock: {status}");
            }
            if Instant::now() > deadline {
                bail!("daemon {expected} never recorded itself as lock holder");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };

    let mut first = spawn_daemon(&free_loopback_bind()?).context("spawn first daemon")?;
    if let Err(error) = wait_for_holder(&mut first) {
        let _ = first.kill();
        let _ = first.wait();
        return Err(error);
    }
    first.kill().context("kill first daemon")?;
    let _ = first.wait();

    let mut second = spawn_daemon(&free_loopback_bind()?).context("spawn second daemon")?;
    let acquired = wait_for_holder(&mut second);
    let second_alive = second.try_wait().context("poll second daemon")?.is_none();
    let _ = second.kill();
    let _ = second.wait();
    kill_synapse_processes_for_db(&db);
    acquired?;
    assert!(
        second_alive,
        "relaunched daemon should keep running after acquiring the guard"
    );
    Ok(())
}

/// Direct WSL interop starts Windows children under the long-lived `wsl.exe`
/// host, so a Windows parent-death watchdog cannot prove the real Linux client
/// lifetime. This is ignored by default because it requires the operator's WSL
/// distro, named by `SYNAPSE_TEST_WSL_DISTRO` (as `wsl -l` lists it); run it on
/// configured Windows hosts before accepting bridge lifecycle changes.
#[test]
#[ignore = "requires a WSL distro named by SYNAPSE_TEST_WSL_DISTRO"]
fn must_fail_direct_wsl_interop_connect_parent_is_refused() -> anyhow::Result<()> {
    let distro = std::env::var("SYNAPSE_TEST_WSL_DISTRO")
        .context("set SYNAPSE_TEST_WSL_DISTRO to the WSL distro name to run this test")?;
    let _guard = lifecycle_test_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let bind = free_loopback_bind()?;
    let wsl_bin = windows_path_to_wsl(bin())?;
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            distro.as_str(),
            "-e",
            "env",
            "SYNAPSE_BEARER_TOKEN=lifecycle-wsl-parent-token",
            &wsl_bin,
            "--mode",
            "connect",
            "--bind",
            &bind,
            "--log-level",
            "info",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("spawn direct WSL interop connect bridge")?;

    assert!(
        !output.status.success(),
        "direct WSL interop bridge should be refused"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MCP_CONNECT_UNSUPPORTED_PARENT"),
        "stderr should name unsupported parent; stderr={stderr}"
    );
    Ok(())
}
