//! Test-owned Win32 windows for the G4 act tests (#7, #62, #107, #153).
#![allow(clippy::unwrap_used, clippy::expect_used)]
//!
//! Each window lives on its own message-pump thread, is shown WITHOUT
//! activation (`SW_SHOWNOACTIVATE`, `WS_EX_NOACTIVATE`, `MA_NOACTIVATE`) and
//! records what the daemon delivered to it:
//! - `wheel_count`: `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL` messages received;
//! - `click_count`: `WM_LBUTTONUP` messages received. Every click also flips
//!   the client fill colour and repaints, so a real click is a real pixel
//!   change of THIS window (the daemon's pixel-delta readback source).
//!
//! The tests never activate these windows and never touch any window they did
//! not create. Drop closes the window and joins its thread.

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

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HGDIOBJ, InvalidateRect,
    PAINTSTRUCT, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GA_ROOT, GetAncestor,
    GetClientRect, GetMessageW, GetWindowRect, IsWindow, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_PAINT, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP, WindowFromPoint,
};
use windows::core::w;

const CLASS_NAME: windows::core::PCWSTR = w!("synapse_g4_act_test_window");
/// Two clearly different fills; a click toggles between them.
const FILLS: [u32; 2] = [0x00F0_C0C0, 0x0030_9030];

#[derive(Default)]
struct Counters {
    wheel: AtomicU32,
    clicks: AtomicU32,
    black: AtomicU32,
}

fn registry() -> &'static Mutex<HashMap<isize, Arc<Counters>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<isize, Arc<Counters>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn counters_for(hwnd: HWND) -> Option<Arc<Counters>> {
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
        // MA_NOACTIVATE = 3: a mouse message never activates the window.
        WM_MOUSEACTIVATE => LRESULT(3),
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            if let Some(counters) = counters_for(hwnd) {
                counters.wheel.fetch_add(1, Ordering::SeqCst);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(counters) = counters_for(hwnd) {
                counters.clicks.fetch_add(1, Ordering::SeqCst);
            }
            let _ = unsafe { InvalidateRect(Some(hwnd), None, true) };
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd);
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
    let clicks = counters_for(hwnd).map_or(0, |c| c.clicks.load(Ordering::SeqCst));
    let fill = if counters_for(hwnd).is_some_and(|c| c.black.load(Ordering::SeqCst) == 1) {
        0
    } else {
        FILLS[(clicks % 2) as usize]
    };
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
    let mut rect = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &raw mut rect) };
    let brush = unsafe { CreateSolidBrush(windows::Win32::Foundation::COLORREF(fill)) };
    let _ = unsafe { FillRect(hdc, &raw const rect, brush) };
    let _ = unsafe { DeleteObject(HGDIOBJ::from(brush)) };
    if counters_for(hwnd).is_some_and(|c| c.black.load(Ordering::SeqCst) == 2) {
        // A genuine black subregion surrounded by rendered colored pixels.
        let inset = RECT {
            left: 32,
            top: 32,
            right: 160,
            bottom: 128,
        };
        let black = unsafe { CreateSolidBrush(windows::Win32::Foundation::COLORREF(0)) };
        let _ = unsafe { FillRect(hdc, &raw const inset, black) };
        let _ = unsafe { DeleteObject(HGDIOBJ::from(black)) };
    }
    let _ = unsafe { EndPaint(hwnd, &raw const ps) };
}

fn register_class() {
    static CLASS: OnceLock<()> = OnceLock::new();
    CLASS.get_or_init(|| {
        let module = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
        let class = WNDCLASSW {
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

/// Placement of a test window in screen pixels.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// `WS_EX_TOPMOST`: the window wins `WindowFromPoint` at its points.
    pub topmost: bool,
}

/// One test-owned, never-activated top-level window.
pub struct OwnedWindow {
    hwnd: isize,
    counters: Arc<Counters>,
    thread: Option<JoinHandle<()>>,
}

impl OwnedWindow {
    pub fn open(placement: Placement) -> Self {
        let counters = Arc::new(Counters::default());
        let (tx, rx) = mpsc::channel::<isize>();
        let thread_counters = Arc::clone(&counters);
        let thread = std::thread::spawn(move || run_window(placement, &thread_counters, &tx));
        let hwnd = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("test window thread reported its HWND");
        let window = Self {
            hwnd,
            counters,
            thread: Some(thread),
        };
        assert!(
            window.is_alive(),
            "precondition: test window 0x{hwnd:x} exists"
        );
        window
    }

    pub const fn hwnd(&self) -> i64 {
        self.hwnd as i64
    }

    /// Screen point at the centre of the window.
    pub fn center(&self) -> (i32, i32) {
        let rect = self.rect();
        ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2)
    }

    /// Window rectangle in physical pixels (the daemon's coordinate space).
    pub fn rect(&self) -> RECT {
        let _physical = PhysicalPixels::enter();
        let mut rect = RECT::default();
        unsafe { GetWindowRect(self.raw(), &raw mut rect) }.expect("GetWindowRect");
        rect
    }

    pub fn wheel_count(&self) -> u32 {
        self.counters.wheel.load(Ordering::SeqCst)
    }

    pub fn click_count(&self) -> u32 {
        self.counters.clicks.load(Ordering::SeqCst)
    }

    /// Known black RGB surface, with normal WGC opaque alpha. #278 regression.
    pub fn paint_black(&self) {
        self.counters.black.store(1, Ordering::SeqCst);
        // SAFETY: only our own live window; the pumping thread repaints it.
        assert!(
            unsafe { RedrawWindow(Some(self.raw()), None, None, RDW_INVALIDATE | RDW_UPDATENOW) }
                .as_bool(),
            "repaint own black fixture"
        );
    }

    /// Black patch inside a colored full window; avoids conflating a valid
    /// black crop with an unavailable whole-window WGC frame (#278 r2).
    pub fn paint_black_region(&self) {
        self.counters.black.store(2, Ordering::SeqCst);
        assert!(
            unsafe { RedrawWindow(Some(self.raw()), None, None, RDW_INVALIDATE | RDW_UPDATENOW) }
                .as_bool()
        );
    }

    pub fn is_alive(&self) -> bool {
        unsafe { IsWindow(Some(self.raw())) }.as_bool()
    }

    /// Polls until `predicate(self)` holds or `timeout` elapses (no fixed sleeps).
    pub fn wait_until(&self, timeout: Duration, predicate: impl Fn(&Self) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        predicate(self)
    }

    fn raw(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }
}

impl Drop for OwnedWindow {
    fn drop(&mut self) {
        let _ = unsafe { PostMessageW(Some(self.raw()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(mut map) = registry().lock() {
            map.remove(&self.hwnd);
        }
    }
}

fn run_window(placement: Placement, counters: &Arc<Counters>, tx: &mpsc::Sender<isize>) {
    register_class();
    let mut ex_style = WS_EX_NOACTIVATE;
    if placement.topmost {
        ex_style |= WS_EX_TOPMOST;
    }
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            CLASS_NAME,
            w!("synapse G4 act test window"),
            WS_POPUP,
            placement.x,
            placement.y,
            placement.w,
            placement.h,
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
        .insert(hwnd.0 as isize, Arc::clone(counters));
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    tx.send(hwnd.0 as isize).expect("report HWND");
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.0 > 0 {
        let _ = unsafe { TranslateMessage(&raw const msg) };
        unsafe { DispatchMessageW(&raw const msg) };
    }
}

/// Root of the current OS foreground window (`None` when there is none, e.g.
/// a locked workstation), read only, for precondition readback.
pub fn foreground_root() -> Option<i64> {
    let foreground = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    if foreground.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(foreground, GA_ROOT) };
    let root = if root.0.is_null() { foreground } else { root };
    Some(root.0 as isize as i64)
}

/// Top-level window under a screen point (same hit test the daemon uses:
/// `WindowFromPoint` + `GetAncestor(GA_ROOT)`), for precondition readback.
/// `(x, y)` are physical pixels, as the per-monitor-aware daemon reads them.
pub fn root_at(x: i32, y: i32) -> Option<i64> {
    let _physical = PhysicalPixels::enter();
    let seed = unsafe { WindowFromPoint(POINT { x, y }) };
    if seed.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(seed, GA_ROOT) };
    let root = if root.0.is_null() { seed } else { root };
    Some(root.0 as isize as i64)
}

/// Per-monitor-v2 DPI context for the calling thread until dropped. The daemon
/// is per-monitor-v2 aware (`synapse_capture::init_process_dpi_awareness`), so
/// screen points this test computes must be physical pixels too; a DPI-unaware
/// test thread would read scaled (logical) coordinates on a 125% display.
struct PhysicalPixels(DPI_AWARENESS_CONTEXT);

impl PhysicalPixels {
    fn enter() -> Self {
        // SAFETY: thread-local DPI context switch; restored on drop.
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for PhysicalPixels {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: restores the context this guard replaced.
            let _restored = unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}
