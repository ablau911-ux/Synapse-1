//! Shared Windows test fixture (G5b raid: #191 #192 #218 #219 #220).
//!
//! Top-level windows created and owned by a dedicated UI thread of THIS test
//! process — never the owner's windows. They are topmost, no-activate tool
//! windows, so creating them never takes the OS foreground. The UI thread pumps
//! messages, so UIA reads, WGC captures and cross-thread `SetWindowTextW` work
//! while the test thread blocks. Wheel messages and barrier tokens that reach
//! the windows are recorded so a test can read back exactly what was delivered.
//!
//! Wiring (manager): in `crates/synapse-mcp/src/server.rs`
//! `#[cfg(all(test, windows))] #[path = "server/tests/own_window_fixture.rs"] pub(crate) mod own_window_fixture;`

use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use synapse_core::Point;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindowRect,
    HWND_TOPMOST, MSG, PostMessageW, PostThreadMessageW, RegisterClassW, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SetWindowPos, SetWindowTextW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_APP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

const CLASS_NAME: PCWSTR = w!("synapse_g5b_own_test_window");
const WAIT: Duration = Duration::from_secs(5);
/// `SS_CENTER | SS_CENTERIMAGE`: label text is drawn at the window centre, so a
/// text change is visible in a point region around the centre.
const LABEL_CENTERED_STYLE: u32 = 0x0001 | 0x0200;

/// One message a fixture window received: a wheel message (`value` = signed
/// wheel delta) or a barrier token (`message == WM_APP`, `value` = token).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowMessage {
    pub(crate) hwnd: i64,
    pub(crate) message: u32,
    pub(crate) value: i64,
}

static MESSAGES: Mutex<Vec<WindowMessage>> = Mutex::new(Vec::new());
static MESSAGES_CHANGED: Condvar = Condvar::new();
static CLASS: OnceLock<Result<(), String>> = OnceLock::new();

/// Screen rectangle of one window, plus an optional full-size STATIC child
/// whose text a test can change (a real UI change on that window).
#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowSpec {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
    pub(crate) label: Option<&'static str>,
}

/// Windows owned by one fixture UI thread; destroyed on drop. Holds the
/// crate-wide desktop-topology guard while the windows exist, so parallel
/// tests never cover each other's topmost windows (T2 raid).
pub(crate) struct OwnWindows {
    thread_id: u32,
    windows: Vec<(i64, Option<i64>)>,
    join: Option<JoinHandle<()>>,
    // Declared last: released only after Drop destroyed the windows.
    _desktop: MutexGuard<'static, ()>,
}

type Created = Result<(u32, Vec<(i64, Option<i64>)>), String>;

impl OwnWindows {
    /// Creates the windows in order (later windows are above earlier ones).
    pub(crate) fn spawn(specs: Vec<WindowSpec>) -> Self {
        let desktop = crate::test_support::desktop_topology_serial();
        let (tx, rx) = mpsc::channel::<Created>();
        let join = std::thread::Builder::new()
            .name("g5b-own-test-windows".to_owned())
            .spawn(move || ui_thread(&specs, &tx))
            .expect("spawn fixture UI thread");
        let (thread_id, windows) = rx
            .recv_timeout(WAIT)
            .expect("fixture windows created in time")
            .expect("fixture windows created");
        Self {
            thread_id,
            windows,
            join: Some(join),
            _desktop: desktop,
        }
    }

    pub(crate) fn hwnd(&self, index: usize) -> i64 {
        self.windows[index].0
    }

    pub(crate) fn label(&self, index: usize) -> i64 {
        self.windows[index]
            .1
            .expect("window was created with a label")
    }

    /// Changes the label text (cross-thread WM_SETTEXT; returns after the UI
    /// thread handled it).
    pub(crate) fn set_label_text(&self, index: usize, text: &str) {
        let text = HSTRING::from(text);
        // SAFETY: the label HWND is live (owned by the fixture thread until drop);
        // the string outlives the synchronous call.
        unsafe { SetWindowTextW(to_hwnd(self.label(index)), PCWSTR(text.as_ptr())) }
            .expect("SetWindowTextW on own label");
    }

    /// Screen point at the centre of the window.
    pub(crate) fn center(&self, index: usize) -> Point {
        let mut rect = RECT::default();
        // SAFETY: live own window; the out-pointer refers to the local `rect`.
        unsafe { GetWindowRect(to_hwnd(self.hwnd(index)), &raw mut rect) }
            .expect("GetWindowRect on own window");
        Point {
            x: (rect.left + rect.right) / 2,
            y: (rect.top + rect.bottom) / 2,
        }
    }

    /// Posts a barrier token to the window and waits until the UI thread has
    /// processed it, so every message posted to it before is processed too.
    pub(crate) fn barrier(&self, index: usize, token: usize) {
        let hwnd = self.hwnd(index);
        // SAFETY: live own window; WM_APP carries only the integer token.
        unsafe { PostMessageW(Some(to_hwnd(hwnd)), WM_APP, WPARAM(token), LPARAM(0)) }
            .expect("post barrier token");
        let expected = WindowMessage {
            hwnd,
            message: WM_APP,
            value: i64::try_from(token).expect("token fits i64"),
        };
        let deadline = Instant::now() + WAIT;
        let mut messages = MESSAGES.lock().expect("messages");
        while !messages.contains(&expected) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "barrier {token} not processed within {WAIT:?}"
            );
            messages = MESSAGES_CHANGED
                .wait_timeout(messages, left)
                .expect("messages wait")
                .0;
        }
    }

    /// Wheel messages (WM_MOUSEWHEEL / WM_MOUSEHWHEEL) the window received.
    pub(crate) fn wheel_messages(&self, index: usize) -> Vec<WindowMessage> {
        let hwnd = self.hwnd(index);
        MESSAGES
            .lock()
            .expect("messages")
            .iter()
            .filter(|m| {
                m.hwnd == hwnd && (m.message == WM_MOUSEWHEEL || m.message == WM_MOUSEHWHEEL)
            })
            .copied()
            .collect()
    }
}

impl Drop for OwnWindows {
    fn drop(&mut self) {
        // SAFETY: the fixture thread owns a message queue (it created windows).
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) fn to_hwnd(hwnd: i64) -> HWND {
    HWND(hwnd as *mut core::ffi::c_void)
}

fn record(message: WindowMessage) {
    MESSAGES.lock().expect("messages").push(message);
    MESSAGES_CHANGED.notify_all();
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let id = hwnd.0 as i64;
    if message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
        let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
        record(WindowMessage {
            hwnd: id,
            message,
            value: i64::from(delta),
        });
        return LRESULT(0);
    }
    if message == WM_APP {
        record(WindowMessage {
            hwnd: id,
            message,
            value: wparam.0 as i64,
        });
        return LRESULT(0);
    }
    // SAFETY: default handling for every other message of our own class.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn register_class() -> Result<(), String> {
    CLASS
        .get_or_init(|| {
            // SAFETY: plain module-handle query for this executable.
            let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: CLASS_NAME,
                hInstance: module.into(),
                ..WNDCLASSW::default()
            };
            // SAFETY: `class` is fully initialised and its name is a static literal.
            if unsafe { RegisterClassW(&raw const class) } == 0 {
                return Err("RegisterClassW failed for the G5b fixture class".to_owned());
            }
            Ok(())
        })
        .clone()
}

fn create(spec: &WindowSpec) -> Result<(i64, Option<i64>), String> {
    register_class()?;
    // SAFETY: own registered class; no Rust pointer is handed to Windows.
    let top = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("synapse G5b own test window"),
            WS_POPUP | WS_BORDER | WS_VISIBLE,
            spec.x,
            spec.y,
            spec.w,
            spec.h,
            None,
            None,
            None,
            None,
        )
    }
    .map_err(|e| format!("CreateWindowExW(top): {e}"))?;
    let label = match spec.label {
        Some(text) => {
            let text = HSTRING::from(text);
            // SAFETY: STATIC is a system class; `top` is a live window on this thread.
            let child = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    PCWSTR(text.as_ptr()),
                    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | LABEL_CENTERED_STYLE),
                    0,
                    0,
                    spec.w,
                    spec.h,
                    Some(top),
                    None,
                    None,
                    None,
                )
            }
            .map_err(|e| format!("CreateWindowExW(label): {e}"))?;
            Some(child.0 as i64)
        }
        None => None,
    };
    // SAFETY: live own window; only z-order/visibility change, no activation.
    unsafe {
        SetWindowPos(
            top,
            Some(HWND_TOPMOST),
            spec.x,
            spec.y,
            spec.w,
            spec.h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .map_err(|e| format!("SetWindowPos: {e}"))?;
    Ok((top.0 as i64, label))
}

fn ui_thread(specs: &[WindowSpec], tx: &mpsc::Sender<Created>) {
    // SAFETY: plain thread-id query.
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut created = Vec::new();
    for spec in specs {
        match create(spec) {
            Ok(window) => created.push(window),
            Err(error) => {
                destroy_all(&created);
                let _ = tx.send(Err(error));
                return;
            }
        }
    }
    let _ = tx.send(Ok((thread_id, created.clone())));
    let mut msg = MSG::default();
    // SAFETY: standard message loop on the thread that owns the windows.
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.0 > 0 {
        // SAFETY: `msg` was filled by GetMessageW above.
        unsafe {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
    destroy_all(&created);
}

fn destroy_all(created: &[(i64, Option<i64>)]) {
    for (hwnd, _) in created {
        // SAFETY: windows created on this thread; destroying the parent destroys its label.
        let _ = unsafe { DestroyWindow(to_hwnd(*hwnd)) };
    }
}
