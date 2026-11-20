//! #144 through the real daemon process: the hidden `SynapseSessionEnd` window must
//! survive session-query messages, and a plain `taskkill /PID` (no `/F`, i.e. WM_CLOSE)
//! must end the daemon with exit code 0 AND an exit-ledger row — never destroy the window
//! and leave the daemon running without OS-shutdown evidence.
//!
//! Each test starts its own `synapse-mcp --mode http` on a free loopback port with a
//! scratch DB/APPDATA and kills it in a Drop guard. No shutdown of the real session.

#![cfg(windows)]

use std::{
    fs,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            FindWindowExW, GetWindowThreadProcessId, IsWindow, SMTO_ABORTIFHUNG,
            SendMessageTimeoutW, WM_ENDSESSION, WM_QUERYENDSESSION,
        },
    },
    core::w,
};

const STARTUP: Duration = Duration::from_secs(30);
const EXIT: Duration = Duration::from_secs(15);

/// One daemon at a time: they share the user's desktop window namespace.
static DAEMON_LOCK: Mutex<()> = Mutex::new(());

// Same kill-and-reap guard as lifecycle_windows.rs, installed at spawn.
struct ChildGuard(Child);

impl std::ops::Deref for ChildGuard {
    type Target = Child;

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

struct Daemon {
    child: ChildGuard,
    _serial: MutexGuard<'static, ()>,
    _tmp: tempfile::TempDir,
    db: PathBuf,
}

impl Daemon {
    fn start() -> Self {
        let serial = DAEMON_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap_or_else(|error| panic!("tmp: {error}"));
        let db = tmp.path().join("db");
        let appdata = tmp.path().join("appdata");
        fs::create_dir_all(appdata.join("synapse")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(appdata.join("synapse").join("token.txt"), "test-token-144")
            .unwrap_or_else(|error| panic!("token: {error}"));
        let logs = tmp.path().join("logs");
        fs::create_dir_all(&logs).unwrap_or_else(|error| panic!("logs: {error}"));
        let child = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
            .args([
                "--mode",
                "http",
                "--bind",
                &free_bind(),
                "--log-level",
                "warn",
                "--db",
            ])
            .arg(&db)
            .env("APPDATA", &appdata)
            .env("SYNAPSE_LOG_DIR", &logs)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(ChildGuard)
            .unwrap_or_else(|error| panic!("spawn daemon: {error}"));
        Self {
            child,
            _serial: serial,
            _tmp: tmp,
            db,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn session_window(&mut self) -> HWND {
        let started = Instant::now();
        loop {
            if let Some(hwnd) = find_session_window(self.pid()) {
                return hwnd;
            }
            assert!(self.running(), "daemon exited during startup");
            assert!(started.elapsed() < STARTUP, "no SynapseSessionEnd window");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn exit_rows(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.db.join("daemon-exit.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

fn free_bind() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error}"));
    let port = listener
        .local_addr()
        .map(|addr| addr.port())
        .unwrap_or_default();
    format!("127.0.0.1:{port}")
}

fn find_session_window(pid: u32) -> Option<HWND> {
    let mut after: Option<HWND> = None;
    loop {
        // SAFETY: plain Win32 enumeration of top-level windows by class name.
        let hwnd = unsafe { FindWindowExW(None, after, w!("SynapseSessionEnd"), None) }.ok()?;
        let mut owner = 0_u32;
        // SAFETY: `owner` is a writable local.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut owner)) };
        if owner == pid {
            return Some(hwnd);
        }
        after = Some(hwnd);
    }
}

fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> usize {
    let mut result = 0_usize;
    // SAFETY: the window belongs to our own child process; result is a writable local.
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            message,
            WPARAM(wparam),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG,
            5_000,
            Some(&raw mut result),
        )
    };
    result
}

fn window_alive(hwnd: HWND) -> bool {
    // SAFETY: IsWindow only reads the handle table.
    unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

fn wait_exit(daemon: &mut Daemon) -> Option<i32> {
    let started = Instant::now();
    while started.elapsed() < EXIT {
        if let Ok(Some(status)) = daemon.child.try_wait() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

#[test]
fn must_pass_session_queries_keep_daemon_and_window_alive_144() {
    let mut daemon = Daemon::start();
    let hwnd = daemon.session_window();

    assert_eq!(
        send(hwnd, WM_QUERYENDSESSION, 0, 0),
        1,
        "shutdown must never be blocked"
    );
    send(hwnd, WM_ENDSESSION, 0, 0); // wParam=0: the session end was cancelled.

    assert!(
        daemon.running(),
        "cancelled session end must not stop the daemon"
    );
    assert!(window_alive(hwnd), "the session window must survive");
    assert!(daemon.exit_rows().is_empty(), "no exit row while running");
}

#[test]
fn must_fail_plain_taskkill_records_close_request_instead_of_destroying_window_144() {
    let mut daemon = Daemon::start();
    let hwnd = daemon.session_window();
    assert!(
        daemon.exit_rows().is_empty(),
        "precondition: no exit row yet"
    );

    let status = Command::new("taskkill")
        .args(["/PID", &daemon.pid().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap_or_else(|error| panic!("taskkill: {error}"));
    assert!(
        status.success(),
        "precondition: taskkill delivered WM_CLOSE"
    );

    let code = wait_exit(&mut daemon);
    if code.is_none() {
        assert!(
            window_alive(hwnd),
            "WM_CLOSE destroyed the session window; daemon still runs"
        );
    }
    assert_eq!(code, Some(0), "WM_CLOSE must end the daemon cleanly");
    let rows = daemon.exit_rows();
    let last = rows
        .last()
        .unwrap_or_else(|| panic!("no daemon-exit.jsonl row"));
    assert_eq!(last["cause"], "close_request", "{last}");
    assert_eq!(last["pid"], daemon.pid(), "{last}");
}

#[test]
fn must_fail_session_end_with_logoff_flag_records_os_logoff_144() {
    const ENDSESSION_LOGOFF: isize = 0x8000_0000;
    let mut daemon = Daemon::start();
    let hwnd = daemon.session_window();

    send(hwnd, WM_ENDSESSION, 1, ENDSESSION_LOGOFF);

    assert_eq!(wait_exit(&mut daemon), Some(0));
    let rows = daemon.exit_rows();
    let last = rows
        .last()
        .unwrap_or_else(|| panic!("no daemon-exit.jsonl row"));
    assert_eq!(last["cause"], "os_logoff", "{last}");
}
