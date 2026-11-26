use std::time::Duration;

use serde_json::json;

use crate::daemon_lifecycle;

const EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Kept outside Runtime::block_on so normal return and unwinding can still run
/// the existing async drain before the runtime is torn down.
pub(crate) struct ProcessExitGuard<'a> {
    runtime: &'a tokio::runtime::Runtime,
}

impl<'a> ProcessExitGuard<'a> {
    pub(crate) fn new(runtime: &'a tokio::runtime::Runtime) -> Self {
        Self { runtime }
    }

    fn finish_unrecorded_exit(&self) -> anyhow::Result<()> {
        if !daemon_lifecycle::exit_marker_pending()? {
            return Ok(());
        }
        if let Err(error) = daemon_lifecycle::record_shutdown_drain(json!({"status": "started"})) {
            eprintln!("synapse-mcp fallback drain diagnostic failed: {error:#}");
        }
        let detail = self.runtime.block_on(async {
            match tokio::time::timeout(
                EXIT_DRAIN_TIMEOUT,
                synapse_a11y::durable_browser_mutation_owners_disable_and_drain(),
            )
            .await
            {
                Ok(readback) => json!({"status": "returned", "readback": readback}),
                Err(_) => json!({
                    "status": "timed_out",
                    "timeout_ms": EXIT_DRAIN_TIMEOUT.as_millis(),
                    "readback": synapse_a11y::durable_browser_mutation_owners_readback(),
                }),
            }
        });
        daemon_lifecycle::record_exit_fallback(
            "process_exit_fallback",
            json!({"graceful": false, "shutdown_drain": detail}),
        )
    }
}

impl Drop for ProcessExitGuard<'_> {
    fn drop(&mut self) {
        // Drop also runs while unwinding. Never let a fallback panic bypass the
        // console-handler completion notification or cause a double panic.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.finish_unrecorded_exit()
        })) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("synapse-mcp exit fallback failed: {error:#}"),
            Err(payload) => {
                let detail = daemon_lifecycle::consume_panic_payload(payload);
                let _ = daemon_lifecycle::record_exit_fallback(
                    "process_exit_fallback_panicked",
                    json!({"graceful": false, "error": detail}),
                );
            }
        }
        #[cfg(windows)]
        windows_shutdown::finish();
    }
}

pub(crate) fn install() -> anyhow::Result<()> {
    #[cfg(windows)]
    windows_shutdown::install()?;
    Ok(())
}

#[cfg(windows)]
pub(crate) async fn requested() {
    windows_shutdown::control().requested.cancelled().await;
}

#[cfg(windows)]
mod windows_shutdown {
    use std::sync::{Condvar, Mutex, OnceLock, mpsc};
    use std::thread::{self, JoinHandle};

    use anyhow::Context as _;
    use tokio_util::sync::CancellationToken;
    use windows::{
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, WPARAM},
            System::{
                Console::{
                    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT,
                    CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
                },
                LibraryLoader::GetModuleHandleW,
                Threading::GetCurrentThreadId,
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
                PostThreadMessageW, RegisterClassW, UnregisterClassW, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_CLOSE, WM_ENDSESSION, WM_QUERYENDSESSION, WM_QUIT, WNDCLASSW,
            },
        },
        core::{BOOL, w},
    };

    use super::*;

    // Windows can terminate the process when a close/session-end callback
    // returns, or after its own deadline. Leave time for a best-effort marker.
    const CONTROL_DRAIN_WAIT: Duration = Duration::from_secs(4);

    struct WindowOwner {
        thread_id: u32,
        thread: JoinHandle<()>,
    }

    pub(super) struct Control {
        pub(super) requested: CancellationToken,
        finished: Mutex<bool>,
        completion: Condvar,
        window: Mutex<Option<WindowOwner>>,
    }

    pub(super) fn control() -> &'static Control {
        static CONTROL: OnceLock<Control> = OnceLock::new();
        CONTROL.get_or_init(|| Control {
            requested: CancellationToken::new(),
            finished: Mutex::new(false),
            completion: Condvar::new(),
            window: Mutex::new(None),
        })
    }

    fn request_and_wait(source: &'static str) {
        let control = control();
        control.requested.cancel();
        let finished = control.finished.lock().unwrap_or_else(|e| e.into_inner());
        let (finished, _) = control
            .completion
            .wait_timeout_while(finished, CONTROL_DRAIN_WAIT, |finished| !*finished)
            .unwrap_or_else(|e| e.into_inner());
        if !*finished {
            // Do not label a missed Windows deadline as graceful or imply the
            // in-flight drain completed. Reuse the lifecycle ledger writer.
            let _ = daemon_lifecycle::record_exit_fallback(
                "windows_shutdown_deadline",
                json!({
                    "source": source,
                    "graceful": false,
                    "shutdown_drain": "requested_but_not_completed_before_windows_deadline",
                }),
            );
        }
    }

    unsafe extern "system" fn console_handler(event: u32) -> BOOL {
        let source = match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                control().requested.cancel();
                return BOOL(1);
            }
            CTRL_CLOSE_EVENT => "CTRL_CLOSE_EVENT",
            CTRL_LOGOFF_EVENT => "CTRL_LOGOFF_EVENT",
            CTRL_SHUTDOWN_EVENT => "CTRL_SHUTDOWN_EVENT",
            _ => return BOOL(0),
        };
        request_and_wait(source);
        BOOL(1)
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CLOSE => {
                // taskkill without /F can deliver WM_CLOSE. Keep the window
                // alive until the transport finishes its existing cleanup.
                control().requested.cancel();
                LRESULT(0)
            }
            WM_QUERYENDSESSION => LRESULT(1),
            WM_ENDSESSION if wparam.0 != 0 => {
                request_and_wait("WM_ENDSESSION");
                LRESULT(0)
            }
            // SAFETY: Forward unhandled messages with their original arguments.
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn window_loop(ready: mpsc::SyncSender<anyhow::Result<u32>>) {
        // A hidden top-level window (not HWND_MESSAGE) receives session-end
        // broadcasts even when user32/gdi32 suppress console logoff/shutdown.
        let instance = match unsafe { GetModuleHandleW(None) } {
            Ok(module) => module.into(),
            Err(error) => {
                let _ = ready.send(Err(error.into()));
                return;
            }
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("SynapseDaemonShutdown"),
            ..Default::default()
        };
        // SAFETY: The class and callback remain valid for this thread's loop.
        if unsafe { RegisterClassW(&class) } == 0 {
            let _ = ready.send(Err(windows::core::Error::from_thread().into()));
            return;
        }
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class.lpszClassName,
                w!("Synapse daemon shutdown"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )
        };
        match window {
            Ok(window) => {
                // SAFETY: This is the window-owning thread and its message queue.
                let thread_id = unsafe { GetCurrentThreadId() };
                if ready.send(Ok(thread_id)).is_ok() {
                    let mut message = MSG::default();
                    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                        unsafe { DispatchMessageW(&message) };
                    }
                }
                unsafe {
                    let _ = DestroyWindow(window);
                }
            }
            Err(error) => {
                let _ = ready.send(Err(error.into()));
            }
        }
        // SAFETY: The owning thread has destroyed its only class instance.
        unsafe {
            let _ = UnregisterClassW(class.lpszClassName, Some(instance));
        }
    }

    pub(super) fn install() -> anyhow::Result<()> {
        let control = control();
        let mut owner = control.window.lock().unwrap_or_else(|e| e.into_inner());
        if owner.is_some() {
            return Ok(());
        }
        let (ready, receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("daemon-shutdown-window".to_owned())
            .spawn(move || window_loop(ready))?;
        match receiver
            .recv()
            .context("receive shutdown window initialization")
        {
            Ok(Ok(thread_id)) => *owner = Some(WindowOwner { thread_id, thread }),
            result => {
                let _ = thread.join();
                return result.and_then(|result| result).map(|_| ());
            }
        }
        // SAFETY: The callback and control state live until process exit.
        unsafe { SetConsoleCtrlHandler(Some(console_handler), true) }
            .context("register Windows console shutdown handler")
    }

    pub(super) fn finish() {
        let control = control();
        *control.finished.lock().unwrap_or_else(|e| e.into_inner()) = true;
        control.completion.notify_all();
        // SAFETY: This removes only our process-lifetime callback.
        let _ = unsafe { SetConsoleCtrlHandler(Some(console_handler), false) };
        let owner = control
            .window
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(owner) = owner {
            // SAFETY: The thread ID came from the initialized window owner.
            let _ = unsafe { PostThreadMessageW(owner.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            let _ = owner.thread.join();
        }
    }
}
