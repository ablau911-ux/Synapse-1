//! OS termination has a much shorter budget than the HTTP graceful drain.
//! Keep the notification thread independent of Tokio and flush evidence before
//! returning to Windows (which can terminate us immediately on return).
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::Context as _;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Console::{
                CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
            },
            LibraryLoader::GetModuleHandleW,
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, ENDSESSION_CLOSEAPP,
            ENDSESSION_LOGOFF, GetMessageW, MSG, PostQuitMessage, RegisterClassW, WINDOW_EX_STYLE,
            WINDOW_STYLE, WM_CLOSE, WM_DESTROY, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW,
        },
    },
    core::{BOOL, w},
};

// Application policy, not a guaranteed OS deadline: spend at most 400ms total
// from the FIRST notification on best-effort evidence. Microsoft's HandlerRoutine
// timeout table lists ordinary logoff/close defaults of 5000ms (configurable),
// and a legacy 500ms "quick" path explicitly described as unused:
// https://learn.microsoft.com/en-us/windows/console/handlerroutine#timeouts
// This conservative cap leaves headroom without holding up session ending.
// Slow storage can leave partial evidence; exit status is not proof of durability.
const EXIT_BUDGET: Duration = Duration::from_millis(400);
static EXIT_REQUESTS: OnceLock<mpsc::Sender<&'static str>> = OnceLock::new();
static EXIT_LATCH: ExitLatch = ExitLatch(OnceLock::new());

#[derive(Default)]
struct ExitLatch(OnceLock<ExitAttempt>);

struct ExitAttempt {
    deadline: Instant,
    completed: AtomicBool,
}

impl ExitLatch {
    fn request(
        &self,
        sender: &mpsc::Sender<&'static str>,
        reason: &'static str,
        budget: Duration,
    ) -> &ExitAttempt {
        self.0.get_or_init(|| {
            let attempt = ExitAttempt {
                deadline: Instant::now() + budget,
                completed: AtomicBool::new(false),
            };
            // Only the first notification queues work. Console and window
            // callbacks share both completion and the original deadline.
            let _ = sender.send(reason);
            attempt
        })
    }
}

impl ExitAttempt {
    fn wait(&self) -> bool {
        while !self.completed.load(Ordering::Acquire) {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(1)));
        }
        true
    }
}

pub(crate) const fn console_reason(event: u32) -> Option<&'static str> {
    match event {
        CTRL_LOGOFF_EVENT => Some("os_logoff"),
        CTRL_SHUTDOWN_EVENT => Some("os_shutdown"),
        // Console window closed (or a non-forced taskkill): not an OS shutdown.
        CTRL_CLOSE_EVENT => Some("console_close"),
        _ => None,
    }
}

pub(crate) const fn session_reason(ending: bool, flags: u32) -> Option<&'static str> {
    if !ending {
        None
    } else if flags & ENDSESSION_LOGOFF != 0 {
        Some("os_logoff")
    } else if flags & ENDSESSION_CLOSEAPP != 0 {
        // Restart Manager / installer servicing asks the app to close; the
        // machine is not shutting down.
        Some("restart_manager")
    } else {
        Some("os_shutdown")
    }
}

/// Window messages that ask the daemon to stop (#144). A plain `taskkill /PID`
/// (no `/F`) posts WM_CLOSE to every top-level window of the process; it means
/// "please exit", so it is a graceful stop request with its own cause. It must
/// never reach DefWindowProc, which would destroy the window and silently
/// disable later OS-shutdown evidence. WM_ENDSESSION needs its parameters and
/// goes through `session_reason`.
pub(crate) const fn window_message_reason(message: u32) -> Option<&'static str> {
    match message {
        WM_CLOSE => Some("close_request"),
        _ => None,
    }
}

fn exit_before_deadline(reason: &'static str) -> ! {
    if let Some(sender) = EXIT_REQUESTS.get() {
        let _ = EXIT_LATCH.request(sender, reason, EXIT_BUDGET).wait();
    }
    // Do not run destructors or the multi-second HTTP/runtime drain here.
    // OS-requested termination is successful even if evidence is partial or
    // storage exceeds our budget. The ledger, not exit code, establishes which
    // writes completed. A decision log alone never proves the ledger was synced.
    std::process::exit(0);
}

// SAFETY: registered via SetConsoleCtrlHandler below, so the Win32 caller supplies a plain u32
// control code; the signature matches PHANDLER_ROUTINE and the body dereferences no pointers.
unsafe extern "system" fn console_handler(event: u32) -> BOOL {
    if let Some(reason) = console_reason(event) {
        exit_before_deadline(reason);
    }
    BOOL(0) // Ctrl+C/Break retain their existing Tokio graceful shutdown path.
}

/// What the hidden session window does with one message (pure dispatch
/// decision behind `window_proc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageAction {
    /// Flush evidence and exit with this cause.
    Exit(&'static str),
    /// The window was destroyed: end this thread's pump so it is recreated.
    RecreateWindow,
    /// Answer the message with this `LRESULT` value.
    Handled(isize),
    /// Forward to `DefWindowProcW`.
    Default,
}

/// Pure dispatch decision for `window_proc`; `wparam`/`lparam` are the raw
/// `WPARAM.0`/`LPARAM.0` values.
pub(crate) const fn message_action(message: u32, wparam: usize, lparam: isize) -> MessageAction {
    if let Some(reason) = window_message_reason(message) {
        return MessageAction::Exit(reason);
    }
    match message {
        // Nothing should destroy this window while the daemon runs. If it
        // happens anyway, end this thread's pump so the owner recreates it.
        WM_DESTROY => MessageAction::RecreateWindow,
        WM_QUERYENDSESSION => MessageAction::Handled(1),
        WM_ENDSESSION => {
            // LPARAM is a bitmask; retain its low 32 bits (including LOGOFF).
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let flags = lparam as u32;
            match session_reason(wparam != 0, flags) {
                Some(reason) => MessageAction::Exit(reason),
                None => MessageAction::Handled(0),
            }
        }
        _ => MessageAction::Default,
    }
}

// SAFETY: installed as `lpfnWndProc` of the window class below, so Windows calls it with the
// WNDPROC signature and valid window-procedure arguments; unhandled messages are forwarded to
// DefWindowProcW unchanged.
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Close requests (#144) exit before any dispatch, independent of
    // `message_action`, so WM_CLOSE can never reach DefWindowProc.
    if let Some(reason) = window_message_reason(message) {
        exit_before_deadline(reason);
    }
    match message_action(message, wp.0, lp.0) {
        MessageAction::Exit(reason) => exit_before_deadline(reason),
        MessageAction::RecreateWindow => {
            tracing::error!(
                code = "MCP_OS_SHUTDOWN_WINDOW_DESTROYED",
                pid = std::process::id(),
                "hidden session-ending window destroyed unexpectedly; recreating"
            );
            // SAFETY: called on the window's owning thread from its window procedure.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        MessageAction::Handled(result) => LRESULT(result),
        // SAFETY: Windows supplies valid window-procedure arguments.
        MessageAction::Default => unsafe { DefWindowProcW(hwnd, message, wp, lp) },
    }
}

fn register_class() -> windows::core::Result<()> {
    // SAFETY: plain Win32 calls; the class only references process-lifetime code.
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: GetModuleHandleW(None)?.into(),
            lpszClassName: w!("SynapseSessionEnd"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
    }
    Ok(())
}

fn create_session_window() -> windows::core::Result<HWND> {
    // SAFETY: called on the thread that pumps this window's messages. No Rust
    // pointer is passed to Windows.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        // Parent=None and no WS_VISIBLE: hidden TOP-LEVEL window.
        // HWND_MESSAGE would never receive session-ending broadcasts.
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("SynapseSessionEnd"),
            w!("SynapseSessionEnd"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

/// Runs until WM_QUIT (posted only by the unexpected-WM_DESTROY path).
fn pump_messages() {
    let mut message = MSG::default();
    // SAFETY: message is writable and this is the owning thread.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).0 > 0 {
            DispatchMessageW(&message);
        }
    }
}

fn recreate_session_window() {
    loop {
        match create_session_window() {
            Ok(hwnd) => {
                tracing::warn!(code = "MCP_OS_SHUTDOWN_WINDOW_RECREATED", pid = std::process::id(),
                    hwnd = ?hwnd, "hidden session-ending window recreated");
                return;
            }
            Err(error) => {
                tracing::error!(code = "MCP_OS_SHUTDOWN_WINDOW_RECREATE_FAILED", %error,
                    "failed to recreate session-ending window; retrying");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

pub(crate) fn install() -> anyhow::Result<()> {
    let (requests, receiver) = mpsc::channel::<&'static str>();
    EXIT_REQUESTS
        .set(requests)
        .map_err(|_| anyhow::anyhow!("OS exit worker already installed"))?;
    std::thread::Builder::new()
        .name("synapse-os-exit".into())
        .spawn(move || {
            if let Ok(reason) = receiver.recv() {
                tracing::info!(
                    code = "MCP_HTTP_PROCESS_EXIT_DECISION",
                    reason,
                    pid = std::process::id(),
                    exit_code = 0,
                    "Windows is ending the daemon process"
                );
                let recorded = crate::daemon_lifecycle::record_startup_exit(
                    reason,
                    serde_json::json!({
                        "code": "MCP_HTTP_PROCESS_EXIT_DECISION", "reason": reason,
                    }),
                );
                if let Err(error) = &recorded {
                    tracing::error!(code = "MCP_DAEMON_LIFECYCLE_EXIT_WRITE_FAILED", %error, reason,
                    "failed to persist OS exit record");
                }
                synapse_telemetry::flush_for_process_exit();
                // Publish only after the flush attempt, including a partial
                // ledger write. Duplicates must not cut the first flush short.
                // wait() also synchronizes with get_or_init publishing the latch.
                EXIT_LATCH.0.wait().completed.store(true, Ordering::Release);
            }
        })
        .context("start OS exit worker")?;

    // Keep console notifications available even if the session window fails.
    // SAFETY: the callback is process-lifetime code with the Win32 signature.
    unsafe { SetConsoleCtrlHandler(Some(console_handler), true) }
        .context("register console exit handler")?;
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::Builder::new().name("synapse-session-window".into()).spawn(move || {
        match register_class().and_then(|()| create_session_window()) {
            Ok(hwnd) => {
                // Intentionally process-lifetime: no normal uninstall path.
                // Windows reclaims this hidden window and class on process exit;
                // DestroyWindow/UnregisterClass here would remove notifications
                // while the HTTP daemon is still serving.
                tracing::info!(code = "MCP_OS_SHUTDOWN_HANDLERS_READY", pid = std::process::id(),
                    hwnd = ?hwnd, "hidden top-level session-ending window ready");
                let _ = ready.send(Ok(()));
                loop {
                    pump_messages();
                    recreate_session_window();
                }
            }
            Err(error) => { let _ = ready.send(Err(error)); }
        }
    }).context("start session-ending window thread")?;
    started
        .recv_timeout(Duration::from_secs(5))
        .context("wait for session-ending window")??;
    Ok(())
}

#[cfg(test)]
#[path = "os_shutdown_close_144_tests.rs"]
mod close_144_tests;
#[cfg(test)]
#[path = "os_shutdown_tests.rs"]
mod tests;
