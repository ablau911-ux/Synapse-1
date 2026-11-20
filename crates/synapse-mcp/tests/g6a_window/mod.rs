//! Test-owned, solid-colour Win32 windows for the G6a capture tests
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! (#48, #67, #102, #103, #122, #224, #47, #49).
//!
//! Every window lives on its own message-pump thread, is shown WITHOUT
//! activation (`WS_EX_NOACTIVATE`, `SW_SHOWNOACTIVATE` / `SW_SHOWMINNOACTIVE`,
//! `MA_NOACTIVATE`) and paints its whole client area with one known colour.
//! The paint handler records the last colour it actually painted, so a test can
//! poll "the window really shows colour X" before triggering a capture.
//!
//! The capture tests only ever capture these synthetic windows, never the
//! owner's real screen content. Drop closes the window and joins its thread.

#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HGDIOBJ, InvalidateRect,
    PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetForegroundWindow, GetMessageW, GetWindowRect, IDC_ARROW, IsIconic, IsWindow, LoadCursorW,
    MA_NOACTIVATE, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_SHOWMINNOACTIVE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos, ShowWindow,
    TranslateMessage, WM_APP, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_PAINT,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_MINIMIZEBOX, WS_POPUP,
};
use windows::core::w;

const CLASS_NAME: windows::core::PCWSTR = w!("synapse_g6a_capture_test_window");
const WM_REPAINT: u32 = WM_APP + 0x61;
const NOT_PAINTED: u32 = u32::MAX;

/// Keep fixture geometry in physical pixels without changing the process DPI mode.
struct PixelDpi(DPI_AWARENESS_CONTEXT);

impl PixelDpi {
    fn enter() -> Self {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        assert!(!previous.0.is_null(), "set fixture thread DPI awareness");
        Self(previous)
    }
}

impl Drop for PixelDpi {
    fn drop(&mut self) {
        let _ = unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

/// Pure colours as `COLORREF` (`0x00BBGGRR`).
pub const RED: u32 = 0x0000_00FF;
pub const GREEN: u32 = 0x0000_FF00;
pub const BLUE: u32 = 0x00FF_0000;

#[derive(Debug)]
struct PaintState {
    color: AtomicU32,
    painted: AtomicU32,
}

fn registry() -> &'static Mutex<HashMap<isize, Arc<PaintState>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<isize, Arc<PaintState>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn state_for(hwnd: HWND) -> Option<Arc<PaintState>> {
    registry()
        .lock()
        .ok()
        .and_then(|map| map.get(&(hwnd.0 as isize)).cloned())
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_REPAINT => {
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint(hwnd: HWND) {
    let Some(state) = state_for(hwnd) else {
        return;
    };
    let color = state.color.load(Ordering::SeqCst);
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
    let mut rect = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &raw mut rect) };
    let brush = unsafe { CreateSolidBrush(COLORREF(color)) };
    let _ = unsafe { FillRect(hdc, &raw const rect, brush) };
    let _ = unsafe { DeleteObject(HGDIOBJ::from(brush)) };
    let _ = unsafe { EndPaint(hwnd, &raw const ps) };
    unsafe { DwmFlush() }.expect("present test window paint to the compositor");
    state.painted.store(color, Ordering::SeqCst);
}

fn register_class() {
    static CLASS: OnceLock<()> = OnceLock::new();
    CLASS.get_or_init(|| {
        let module = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
        // #2399 G16: window capture includes the pointer. A class without a
        // cursor leaves Windows' animated busy ring over the fixture when the
        // pointer rests there, and its frames are real changes in a window the
        // tests treat as static. A static arrow keeps the fixture's pixels still.
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.expect("load the system arrow cursor");
        let class = WNDCLASSW {
            hCursor: cursor,
            lpfnWndProc: Some(wnd_proc),
            lpszClassName: CLASS_NAME,
            hInstance: module.into(),
            ..WNDCLASSW::default()
        };
        assert_ne!(
            unsafe { RegisterClassW(&raw const class) },
            0,
            "RegisterClassW"
        );
    });
}

/// How the window is first shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    Normal,
    Minimized,
}

/// Placement, colour and show state of a test window (screen pixels).
#[derive(Clone, Copy, Debug)]
pub struct WindowSpec {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub color: u32,
    pub show: Show,
}

impl WindowSpec {
    /// A visible window at a fixed, unobtrusive spot.
    pub const fn visible(w: i32, h: i32, color: u32) -> Self {
        Self {
            x: 60,
            y: 60,
            w,
            h,
            color,
            show: Show::Normal,
        }
    }

    /// A window that starts minimized (WGC never receives a frame for it).
    pub const fn minimized(color: u32) -> Self {
        Self {
            x: 60,
            y: 60,
            w: 320,
            h: 240,
            color,
            show: Show::Minimized,
        }
    }
}

/// One test-owned, never-activated top-level window painted in one colour.
pub struct PaintedWindow {
    hwnd: isize,
    state: Arc<PaintState>,
    thread: Option<JoinHandle<()>>,
}

impl PaintedWindow {
    pub fn open(spec: WindowSpec) -> Self {
        let state = Arc::new(PaintState {
            color: AtomicU32::new(spec.color),
            painted: AtomicU32::new(NOT_PAINTED),
        });
        let (tx, rx) = mpsc::channel::<isize>();
        let thread_state = Arc::clone(&state);
        let thread = std::thread::spawn(move || run_window(spec, &thread_state, &tx));
        let hwnd = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("test window thread reported its HWND");
        let window = Self {
            hwnd,
            state,
            thread: Some(thread),
        };
        assert!(
            window.is_alive(),
            "precondition: test window {hwnd:#x} exists"
        );
        if spec.show == Show::Normal {
            assert!(
                window.wait_painted(spec.color, Duration::from_secs(5)),
                "precondition: test window {hwnd:#x} painted {:#08x}",
                spec.color
            );
        } else {
            assert!(
                window.is_minimized(),
                "precondition: test window {hwnd:#x} is minimized"
            );
        }
        window
    }

    pub const fn hwnd(&self) -> i64 {
        self.hwnd as i64
    }

    fn raw(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    /// Changes the fill colour and asks the window thread to repaint.
    pub fn set_color(&self, color: u32) {
        self.state.color.store(color, Ordering::SeqCst);
        let _ = unsafe { PostMessageW(Some(self.raw()), WM_REPAINT, WPARAM(0), LPARAM(0)) };
    }

    /// Polls (no fixed sleep) until the window has really painted `color`.
    pub fn wait_painted(&self, color: u32, timeout: Duration) -> bool {
        wait_until(timeout, || {
            self.state.painted.load(Ordering::SeqCst) == color
        })
    }

    /// Wait for WGC to see the painted surface, not only a completed WM_PAINT.
    pub fn wait_captured_color(&self, color: u32, timeout: Duration) -> bool {
        let capture = synapse_capture::spawn_capture_loop(synapse_capture::CaptureConfig {
            target: synapse_capture::CaptureTarget::Window { hwnd: self.hwnd() },
            min_update_interval_ms: 16,
            cursor_visible: false,
            dirty_region_only: false,
            ..synapse_capture::CaptureConfig::default()
        })
        .expect("start fixture presentation readback");
        let receiver = capture.receiver();
        let ready = wait_until(timeout, || {
            let Ok(frame) = receiver.try_recv() else {
                return false;
            };
            if frame.width == 0 || frame.height == 0 {
                return false;
            }
            let offset = frame.height as usize / 2 * frame.pixels.row_stride_bytes
                + frame.width as usize / 2 * 4;
            let pixel = &frame.pixels.bytes[offset..offset + 4];
            let rgb = match frame.format {
                synapse_capture::DxgiFormat::Rgba8 | synapse_capture::DxgiFormat::Rgba8Srgb => {
                    [pixel[0], pixel[1], pixel[2]]
                }
                _ => [pixel[2], pixel[1], pixel[0]],
            };
            classify_rgb(rgb) == Some(color)
        });
        capture.stop().expect("stop fixture presentation readback");
        ready
    }

    /// Resizes without moving or activating; returns the `GetWindowRect` readback.
    pub fn resize(&self, w: i32, h: i32) -> RECT {
        let _dpi = PixelDpi::enter();
        unsafe {
            SetWindowPos(
                self.raw(),
                None,
                0,
                0,
                w,
                h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .expect("SetWindowPos resize");
        self.rect()
    }

    pub fn rect(&self) -> RECT {
        let _dpi = PixelDpi::enter();
        let mut rect = RECT::default();
        unsafe { GetWindowRect(self.raw(), &raw mut rect) }.expect("GetWindowRect");
        rect
    }

    pub fn minimize(&self) {
        let _ = unsafe { ShowWindow(self.raw(), SW_SHOWMINNOACTIVE) };
    }

    pub fn is_minimized(&self) -> bool {
        unsafe { IsIconic(self.raw()) }.as_bool()
    }

    pub fn is_alive(&self) -> bool {
        unsafe { IsWindow(Some(self.raw())) }.as_bool()
    }

    /// Closes the window now (the capture target disappears) and joins its thread.
    pub fn destroy(&mut self) {
        let _ = unsafe { PostMessageW(Some(self.raw()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(mut map) = registry().lock() {
            map.remove(&self.hwnd);
        }
    }
}

impl Drop for PaintedWindow {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn run_window(spec: WindowSpec, state: &Arc<PaintState>, tx: &mpsc::Sender<isize>) {
    let _dpi = PixelDpi::enter();
    register_class();
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("synapse G6a capture test window"),
            WS_POPUP | WS_MINIMIZEBOX,
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
    .expect("CreateWindowExW test window");
    registry()
        .lock()
        .expect("window registry")
        .insert(hwnd.0 as isize, Arc::clone(state));
    let show = match spec.show {
        Show::Normal => SW_SHOWNOACTIVATE,
        Show::Minimized => SW_SHOWMINNOACTIVE,
    };
    let _ = unsafe { ShowWindow(hwnd, show) };
    tx.send(hwnd.0 as isize).expect("report HWND");
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.0 > 0 {
        let _ = unsafe { TranslateMessage(&raw const msg) };
        unsafe { DispatchMessageW(&raw const msg) };
    }
}

/// Raw HWND of the current OS foreground window (0 when none, e.g. locked).
pub fn foreground_hwnd() -> i64 {
    unsafe { GetForegroundWindow() }.0 as i64
}

/// Polls `predicate` every 5 ms until it holds or `timeout` elapses.
pub fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    predicate()
}

/// Nearest pure colour (`RED`/`GREEN`/`BLUE`) of an RGB pixel, or `None` when
/// the pixel is not clearly one of them (GIF palette quantization tolerated).
pub fn classify_rgb(rgb: [u8; 3]) -> Option<u32> {
    let [r, g, b] = rgb;
    match (r > 160, g > 160, b > 160, r < 90, g < 90, b < 90) {
        (true, false, false, _, true, true) => Some(RED),
        (false, true, false, true, _, true) => Some(GREEN),
        (false, false, true, true, true, _) => Some(BLUE),
        _ => None,
    }
}
