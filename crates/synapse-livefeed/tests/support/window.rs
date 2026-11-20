#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(unsafe_code)]

use std::cell::Cell;
use std::collections::BTreeSet;
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use synapse_core::Rect;
use windows::Win32::Foundation::{
    COLORREF, CloseHandle, HLOCAL, HWND, LPARAM, LRESULT, LocalFree, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetDC, GetPixel,
    InvalidateRect, PAINTSTRUCT, ReleaseDC, UpdateWindow,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, GetThreadDescription, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    GetWindowRect, IDC_ARROW, LoadCursorW, MSG, PostThreadMessageW, RegisterClassW, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, TranslateMessage, WM_APP, WM_PAINT, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

const PAINT_COLOR: u32 = WM_APP + 0x244;
const WAIT: Duration = Duration::from_secs(10);
const RECTANGLE: Rect = Rect {
    x: 64,
    y: 64,
    // Keep a nonempty interior after excluding a full 64px tile on each side.
    w: 256,
    h: 256,
};
static CLASS: OnceLock<()> = OnceLock::new();
// Keep the window and its owner thread alive until process exit (capture bug #261).
static WINDOW: OnceLock<Window> = OnceLock::new();
thread_local! {
    static COLOR: Cell<[u8; 3]> = const { Cell::new(super::FIRST) };
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

unsafe extern "system" fn wndproc(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message != WM_PAINT {
        // SAFETY: forwarding unchanged parameters on the owning message thread.
        return unsafe { DefWindowProcW(window, message, wp, lp) };
    }
    let mut paint = PAINTSTRUCT::default();
    let [r, g, b] = COLOR.get();
    let rgb = COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16));
    let rect = RECT {
        left: 0,
        top: 0,
        right: RECTANGLE.w,
        bottom: RECTANGLE.h,
    };
    // SAFETY: BeginPaint/EndPaint pair on own HWND; brush deleted after use.
    unsafe {
        let dc = BeginPaint(window, &raw mut paint);
        let brush = CreateSolidBrush(rgb);
        FillRect(dc, &raw const rect, brush);
        let _ = DeleteObject(brush.into());
        let _ = EndPaint(window, &raw const paint);
    }
    LRESULT(0)
}

fn create_window() -> HWND {
    CLASS.get_or_init(|| {
        // #2399 G15: the camera captures the mouse pointer. A class without a
        // cursor leaves Windows' animated busy ring over the fixture when the
        // pointer rests there, and its spinning frames are real changes inside
        // the window. A static arrow keeps the fixture's own pixels unchanged.
        // SAFETY: loads a shared system cursor; nothing is freed or written.
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.expect("load the system arrow cursor");
        let class = WNDCLASSW {
            hCursor: cursor,
            lpfnWndProc: Some(wndproc),
            lpszClassName: w!("synapse_livefeed_camera_tests"),
            ..WNDCLASSW::default()
        };
        // SAFETY: static class name and procedure live for the process lifetime.
        assert_ne!(unsafe { RegisterClassW(&raw const class) }, 0);
    });
    // SAFETY: owner thread creates the window. NOACTIVATE prevents focus theft.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("synapse_livefeed_camera_tests"),
            w!("Livefeed synthetic camera test"),
            WS_POPUP | WS_VISIBLE,
            RECTANGLE.x,
            RECTANGLE.y,
            RECTANGLE.w,
            RECTANGLE.h,
            None,
            None,
            None,
            None,
        )
    }
    .expect("create non-activating synthetic window");
    repaint(window);
    window
}

fn repaint(window: HWND) {
    // SAFETY: synchronous repaint on the window's owner thread.
    unsafe {
        assert!(InvalidateRect(Some(window), None, false).as_bool());
        assert!(UpdateWindow(window).as_bool());
    }
}

fn ui_thread(
    ready: mpsc::SyncSender<(isize, u32)>,
    colors: mpsc::Receiver<([u8; 3], mpsc::SyncSender<()>)>,
) {
    // SAFETY: affects only this process-lifetime fixture thread.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let window = create_window();
    // SAFETY: query of the current thread identifier.
    ready
        .send((window.0 as isize, unsafe { GetCurrentThreadId() }))
        .unwrap();
    let mut message = MSG::default();
    // SAFETY: standard owner-thread pump; no human input is sent.
    loop {
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) }.0;
        assert_ne!(result, -1, "fixture message pump failed");
        if result == 0 {
            // Even an unexpected quit message must not end the window's lifetime.
            continue;
        }
        if message.hwnd.0.is_null() && message.message == PAINT_COLOR {
            for (color, reply) in colors.try_iter() {
                COLOR.set(color);
                repaint(window);
                let _ = reply.send(());
            }
        } else {
            // SAFETY: dispatching the message just received by this thread.
            unsafe {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }
}

#[derive(Clone)]
pub struct Window {
    raw: isize,
    thread_id: u32,
    colors: mpsc::Sender<([u8; 3], mpsc::SyncSender<()>)>,
}

impl Window {
    pub fn spawn() -> Self {
        let window = WINDOW
            .get_or_init(|| {
                let (ready_tx, ready_rx) = mpsc::sync_channel(1);
                let (colors, receiver) = mpsc::channel();
                // Dropping the join handle detaches this process-lifetime thread.
                std::thread::Builder::new()
                    .name("livefeed-test-window".into())
                    .spawn(move || ui_thread(ready_tx, receiver))
                    .expect("spawn window thread");
                let (raw, thread_id) = ready_rx.recv_timeout(WAIT).expect("synthetic window ready");
                Self {
                    raw,
                    thread_id,
                    colors,
                }
            })
            .clone();
        window.move_to(RECTANGLE.x, RECTANGLE.y);
        window.paint(super::FIRST);
        window
    }

    pub fn rect(&self) -> Rect {
        // SAFETY: query the live persistent HWND in physical screen pixels.
        unsafe {
            let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let mut rect = RECT::default();
            let result = GetWindowRect(hwnd(self.raw), &raw mut rect);
            SetThreadDpiAwarenessContext(previous);
            result.expect("read fixture's physical rectangle");
            Rect {
                x: rect.left,
                y: rect.top,
                w: rect.right - rect.left,
                h: rect.bottom - rect.top,
            }
        }
    }

    pub fn move_to(&self, x: i32, y: i32) {
        // SAFETY: move only our persistent window without activation or resizing.
        unsafe {
            let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let result = SetWindowPos(
                hwnd(self.raw),
                None,
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            );
            SetThreadDpiAwarenessContext(previous);
            result.expect("move fixture without activating it");
        }
        self.assert_not_foreground();
    }

    pub fn paint(&self, color: [u8; 3]) {
        let (tx, rx) = mpsc::sync_channel(1);
        self.colors.send((color, tx)).expect("queue color");
        // SAFETY: fixture thread owns a live message queue.
        unsafe { PostThreadMessageW(self.thread_id, PAINT_COLOR, WPARAM(0), LPARAM(0)) }.unwrap();
        rx.recv_timeout(WAIT)
            .expect("owner thread painted requested color");
        self.assert_not_foreground();
    }

    pub fn assert_not_foreground(&self) {
        // SAFETY: read-only foreground query; this fixture never activates a window.
        assert_ne!(
            unsafe { GetForegroundWindow() },
            hwnd(self.raw),
            "fixture stole focus"
        );
    }
}

pub fn screen_pixel(rect: Rect) -> [u8; 3] {
    // SAFETY: acquire/release screen DC here; no input or focus changes.
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let dc = GetDC(None);
        assert!(!dc.0.is_null());
        let value = GetPixel(dc, rect.x + rect.w / 2, rect.y + rect.h / 2).0;
        ReleaseDC(None, dc);
        SetThreadDpiAwarenessContext(previous);
        assert_ne!(value, u32::MAX, "physical screen pixel unavailable");
        [value as u8, (value >> 8) as u8, (value >> 16) as u8]
    }
}

pub fn thread_ids() -> BTreeSet<u32> {
    let mut ids = BTreeSet::new();
    // SAFETY: read-only thread snapshot, closed on completion.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).unwrap();
        let mut row = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        Thread32First(snapshot, &raw mut row).expect("enumerate process threads");
        loop {
            if row.th32OwnerProcessID == std::process::id() {
                ids.insert(row.th32ThreadID);
            }
            if Thread32Next(snapshot, &raw mut row).is_err() {
                break;
            }
        }
        CloseHandle(snapshot).unwrap();
    }
    ids
}

pub fn thread_description(id: u32) -> Option<String> {
    // SAFETY: query-only handle to this process's enumerated thread; both the
    // handle and the OS-allocated description are released before returning.
    unsafe {
        let handle = match OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, id) {
            Ok(handle) => handle,
            // A pool thread may exit between the snapshot and OpenThread.
            Err(error) if error.code() == windows::core::HRESULT::from_win32(87) => return None,
            Err(error) => panic!("open thread {id} for description: {error}"),
        };
        let description = GetThreadDescription(handle);
        CloseHandle(handle).expect("close queried thread handle");
        let description = description.expect("read thread description");
        let name = description.to_string();
        let _ = LocalFree(Some(HLOCAL(description.0.cast())));
        Some(name.expect("thread description is UTF-16"))
    }
}
