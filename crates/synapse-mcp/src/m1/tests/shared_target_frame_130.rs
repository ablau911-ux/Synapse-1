//! Regression tests for #130:
//! 1. `populate_window_capture_baseline` and `populate_luanti_visible_baseline`
//!    must read the target HWND's own WGC surface rather than desktop screen
//!    pixels (`screen_region_to_bgra_bitmap`), and fail closed on invalid HWNDs.
//! 2. Within a single observation (`ObservationInput`), baseline capture,
//!    `browser_ocr_target_frame`, and `populate_detection_from_state` must share
//!    the same captured target frame rather than re-capturing live across stages.

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

use synapse_core::{PerceptionMode, Rect, SensorStatus, error_codes};
use synapse_perception::ObservationInput;
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Dwm::DwmFlush,
            Gdi::{
                BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HGDIOBJ,
                InvalidateRect, PAINTSTRUCT,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
            GetMessageW, HWND_BOTTOM, HWND_TOPMOST, IsIconic, IsWindow, MA_NOACTIVATE, MSG,
            PostMessageW, PostQuitMessage, RegisterClassW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
            TranslateMessage, WM_APP, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_MOUSEACTIVATE,
            WM_PAINT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_MINIMIZEBOX, WS_POPUP,
        },
    },
    core::w,
};

use super::*;

const CLASS_NAME: windows::core::PCWSTR = w!("synapse_g6b_130_test_window");
const WM_REPAINT: u32 = WM_APP + 0x30;
const SPLIT_BW: u32 = 0x8000_0000;
const RED: u32 = 0x0000_00FF;
const BLUE: u32 = 0x00FF_0000;
const FLAT_GRAY: u32 = 0x0040_4040;

struct WinState {
    mode: AtomicU32,
    painted: AtomicU32,
}

fn registry() -> &'static Mutex<HashMap<isize, Arc<WinState>>> {
    static REG: OnceLock<Mutex<HashMap<isize, Arc<WinState>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
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
            paint_window(hwnd);
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

fn paint_window(hwnd: HWND) {
    let Some(state) = registry()
        .lock()
        .ok()
        .and_then(|m| m.get(&(hwnd.0 as isize)).cloned())
    else {
        return;
    };
    let mode = state.mode.load(Ordering::SeqCst);
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &raw mut rc) };
    if mode == SPLIT_BW {
        let mid_y = (rc.top + rc.bottom) / 2;
        let top_rc = RECT {
            bottom: mid_y,
            ..rc
        };
        let bot_rc = RECT { top: mid_y, ..rc };
        fill_rect(hdc, &top_rc, 0x0000_0000);
        fill_rect(hdc, &bot_rc, 0x00FF_FFFF);
    } else {
        fill_rect(hdc, &rc, mode);
    }
    let _ = unsafe { EndPaint(hwnd, &raw const ps) };
    state.painted.store(mode, Ordering::SeqCst);
}

fn fill_rect(hdc: windows::Win32::Graphics::Gdi::HDC, rc: &RECT, color: u32) {
    let brush = unsafe { CreateSolidBrush(COLORREF(color)) };
    let _ = unsafe { FillRect(hdc, rc, brush) };
    let _ = unsafe { DeleteObject(HGDIOBJ::from(brush)) };
}

fn register_class() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let module = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            lpszClassName: CLASS_NAME,
            hInstance: module.into(),
            ..WNDCLASSW::default()
        };
        assert_ne!(unsafe { RegisterClassW(&raw const class) }, 0);
    });
}

struct TestWindow {
    hwnd: isize,
    state: Arc<WinState>,
    thread: Option<JoinHandle<()>>,
}

impl TestWindow {
    fn open(bounds: Rect, mode: u32, topmost: bool) -> Self {
        // Mirror daemon startup (main.rs): per-monitor-v2 process, so window bounds,
        // screen reads and WGC frames all use physical pixels regardless of test order.
        synapse_capture::init_process_dpi_awareness().expect("init process DPI awareness");
        let state = Arc::new(WinState {
            mode: AtomicU32::new(mode),
            painted: AtomicU32::new(u32::MAX),
        });
        let (tx, rx) = mpsc::channel::<isize>();
        let thread_state = Arc::clone(&state);
        let thread = std::thread::spawn(move || run_loop(bounds, topmost, &thread_state, &tx));
        let hwnd = rx.recv_timeout(Duration::from_secs(5)).expect("hwnd");
        let win = Self {
            hwnd,
            state,
            thread: Some(thread),
        };
        assert!(win.wait_until(|| win.state.painted.load(Ordering::SeqCst) == mode));
        win
    }

    const fn hwnd(&self) -> i64 {
        self.hwnd as i64
    }

    fn raw(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    fn set_mode(&self, mode: u32) {
        self.state.mode.store(mode, Ordering::SeqCst);
        let _ = unsafe { PostMessageW(Some(self.raw()), WM_REPAINT, WPARAM(0), LPARAM(0)) };
        assert!(self.wait_until(|| self.state.painted.load(Ordering::SeqCst) == mode));
    }

    fn ensure_topmost(&self) {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        let _ = unsafe { SetWindowPos(self.raw(), Some(HWND_TOPMOST), 0, 0, 0, 0, flags) };
    }

    fn minimize(&self) {
        let _ = unsafe { ShowWindow(self.raw(), SW_SHOWMINNOACTIVE) };
        assert!(self.wait_until(|| unsafe { IsIconic(self.raw()) }.as_bool()));
    }

    fn wait_until(&self, mut pred: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        pred()
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        if unsafe { IsWindow(Some(self.raw())) }.as_bool() {
            let _ = unsafe { PostMessageW(Some(self.raw()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(mut map) = registry().lock() {
            map.remove(&self.hwnd);
        }
    }
}

fn run_loop(bounds: Rect, topmost: bool, state: &Arc<WinState>, tx: &mpsc::Sender<isize>) {
    register_class();
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("Luanti 5.10.0"),
            WS_POPUP | WS_MINIMIZEBOX,
            bounds.x,
            bounds.y,
            bounds.w,
            bounds.h,
            None,
            None,
            None,
            None,
        )
    }
    .expect("CreateWindowExW");
    registry()
        .lock()
        .expect("registry")
        .insert(hwnd.0 as isize, Arc::clone(state));
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    let z = if topmost { HWND_TOPMOST } else { HWND_BOTTOM };
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            Some(z),
            bounds.x,
            bounds.y,
            bounds.w,
            bounds.h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    tx.send(hwnd.0 as isize).expect("send hwnd");
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.0 > 0 {
        let _ = unsafe { TranslateMessage(&raw const msg) };
        unsafe { DispatchMessageW(&raw const msg) };
    }
}

fn luanti_input(hwnd: i64, bounds: Rect) -> ObservationInput {
    let mut fg = foreground_with_bounds(bounds);
    fg.hwnd = hwnd;
    fg.process_name = "luanti.exe".to_owned();
    fg.window_title = "Luanti 5.10.0".to_owned();
    ObservationInput::new(fg)
}

/// Physical screen bounds of the live window. Creation coordinates are
/// logical in this DPI-unaware test process (240 px becomes 300 px at 125 %),
/// while capture and crop work in physical pixels, like the PMv2 daemon.
fn live_bounds(win: &TestWindow) -> Rect {
    synapse_capture::window_capture_bounds(win.hwnd()).expect("live window capture bounds")
}

fn center_rb(bitmap: &synapse_capture::CapturedBgraBitmap) -> (u8, u8) {
    let cx = (bitmap.width / 2) as usize;
    let cy = (bitmap.height / 2) as usize;
    let idx = (cy * (bitmap.width as usize) + cx) * 4;
    (bitmap.bytes[idx + 2], bitmap.bytes[idx])
}

/// Screen spot for the occluded-target pair, the only #130 case that reads
/// desktop pixels. Every other #130 test (and most window tests in this crate)
/// opens topmost windows at 80,80; running in parallel they cover this pair's
/// crosshair and break the covering-pixel precondition. A spot no other test
/// uses keeps the precondition about this test's own windows only. Logical
/// coordinates inside the 1536x864 logical desktop (1920x1080 at 125 %).
const OCCLUDED_PAIR_BOUNDS: Rect = Rect {
    x: 1180,
    y: 520,
    w: 240,
    h: 240,
};

#[test]
#[ignore = "exclusive visible desktop: needs a topmost cover window nothing else can demote; run serially: cargo test -p synapse-mcp --bin synapse-mcp -- --ignored --test-threads=1 shared_target_frame_130"]
fn must_fail_occluded_luanti_target_does_not_read_covering_window_pixels() {
    // Real topmost windows + screen reads: serialize with the other desktop tests.
    let _desktop = crate::test_support::desktop_topology_serial();
    let bounds = OCCLUDED_PAIR_BOUNDS;
    let target = TestWindow::open(bounds, FLAT_GRAY, false);
    let cover = TestWindow::open(bounds, SPLIT_BW, true);
    // The new window may not have painted yet: an unpainted surface is all zero.
    assert!(
        target.wait_until(|| {
            synapse_capture::window_full_frame_to_bgra_bitmap(target.hwnd(), 1_000).is_ok()
        }),
        "precondition: target WGC frame once the window has painted"
    );
    let target_wgc = synapse_capture::window_full_frame_to_bgra_bitmap(target.hwnd(), 1_000)
        .expect("precondition: target WGC frame");
    assert!(
        bgra_contrast_score(&target_wgc.bitmap.bytes) <= 0.02,
        "precondition: target own surface is flat gray"
    );
    let cross = centered_region(live_bounds(&cover), 48, 48).expect("crosshair");
    // Another test in this binary can demote this topmost window (observed while
    // the #95 K2 tests ran concurrently: WS_EX_TOPMOST gone, VS Code on top), so
    // re-assert topmost before each screen read instead of trusting creation.
    assert!(
        cover.wait_until(|| {
            cover.ensure_topmost();
            // WM_PAINT only submits pixels; wait for DWM to compose the cover
            // before sampling the physical desktop. Keep the bounded pixel
            // retry: a cover that never composites must still fail closed.
            if unsafe { DwmFlush() }.is_err() {
                return false;
            }
            synapse_capture::screen_region_to_bgra_bitmap(cross)
                .map(|b| bgra_contrast_score(&b.bytes) > 0.35)
                .unwrap_or(false)
        }),
        "precondition: covering window supplies high-contrast screen pixels"
    );

    let mut input = luanti_input(target.hwnd(), live_bounds(&target));
    populate_window_capture_baseline(&mut input);
    let reading = input
        .hud
        .by_name
        .get("luanti.crosshair_contrast")
        .expect("crosshair contrast reading");
    assert!(
        reading.confidence <= 0.05,
        "occluded Luanti target leaked covering window contrast: {}",
        reading.confidence
    );
}

#[test]
fn must_fail_invalid_hwnd_capture_baseline_fails_closed_without_screen_read() {
    // Real topmost windows + screen reads: serialize with the other desktop tests.
    let _desktop = crate::test_support::desktop_topology_serial();
    let bounds = Rect {
        x: 80,
        y: 80,
        w: 240,
        h: 240,
    };
    let _visible = TestWindow::open(bounds, SPLIT_BW, true);
    let mut input = luanti_input(0x7fff_dead, bounds);
    populate_window_capture_baseline(&mut input);
    assert!(
        matches!(
            input.capture_status,
            SensorStatus::DegradedSensorFailed { .. }
        ),
        "invalid target HWND must fail capture baseline closed, got {:?}",
        input.capture_status
    );
    assert!(input.hud.by_name.is_empty());
    assert!(input.entities.is_empty());
}

#[test]
fn must_pass_visible_luanti_target_reads_its_own_surface_contrast() {
    // Real topmost windows + screen reads: serialize with the other desktop tests.
    let _desktop = crate::test_support::desktop_topology_serial();
    let bounds = Rect {
        x: 80,
        y: 80,
        w: 240,
        h: 240,
    };
    let win = TestWindow::open(bounds, SPLIT_BW, true);
    assert!(
        win.wait_until(|| {
            synapse_capture::window_full_frame_to_bgra_bitmap(win.hwnd(), 1_000)
                .map(|c| bgra_contrast_score(&c.bitmap.bytes) > 0.35)
                .unwrap_or(false)
        }),
        "precondition: target WGC frame has split black/white contrast"
    );
    let mut input = luanti_input(win.hwnd(), live_bounds(&win));
    populate_window_capture_baseline(&mut input);
    assert_eq!(input.capture_status, SensorStatus::Healthy);
    let reading = input
        .hud
        .by_name
        .get("luanti.crosshair_contrast")
        .expect("crosshair reading");
    assert!(reading.confidence > 0.35);
    assert_eq!(input.entities.len(), 2);
}

#[test]
fn must_fail_browser_ocr_reuses_initial_observation_frame_after_window_repaints() {
    // Real topmost windows + screen reads: serialize with the other desktop tests.
    let _desktop = crate::test_support::desktop_topology_serial();
    let bounds = Rect {
        x: 80,
        y: 80,
        w: 240,
        h: 240,
    };
    let win = TestWindow::open(bounds, RED, true);
    assert!(
        win.wait_until(|| {
            synapse_capture::window_full_frame_to_bgra_bitmap(win.hwnd(), 1_000)
                .map(|c| {
                    let (r, b) = center_rb(&c.bitmap);
                    r > 200 && b < 40
                })
                .unwrap_or(false)
        }),
        "precondition: initial WGC frame is red"
    );

    let input = window_input_from_hwnd(win.hwnd(), 2, PerceptionMode::Hybrid)
        .expect("initial window input");
    assert_eq!(input.capture_status, SensorStatus::Healthy);

    win.set_mode(BLUE);
    assert!(
        win.wait_until(|| {
            synapse_capture::window_full_frame_to_bgra_bitmap(win.hwnd(), 1_000)
                .map(|c| {
                    let (r, b) = center_rb(&c.bitmap);
                    b > 200 && r < 40
                })
                .unwrap_or(false)
        }),
        "precondition: live window repainted blue"
    );

    let ocr_frame = crate::m1::browser_ocr_target_frame(&input)
        .expect("browser OCR target frame from shared observation capture");
    let (r, b) = center_rb(&ocr_frame);
    assert!(
        r > 200 && b < 40,
        "browser OCR must reuse the observation's initial red frame, got (r={r}, b={b})"
    );
}

#[test]
fn must_fail_detection_reuses_initial_observation_frame_after_window_minimizes() {
    // Real topmost windows + screen reads: serialize with the other desktop tests.
    let _desktop = crate::test_support::desktop_topology_serial();
    let bounds = Rect {
        x: 80,
        y: 80,
        w: 240,
        h: 240,
    };
    let win = TestWindow::open(bounds, RED, true);
    let mut input = window_input_from_hwnd(win.hwnd(), 2, PerceptionMode::PixelOnly)
        .expect("initial window input");
    assert_eq!(input.capture_status, SensorStatus::Healthy);

    win.minimize();

    let mut state = crate::m1::M1State::from_env();
    state.perception_mode = PerceptionMode::PixelOnly;
    state.detection_config.model_id = Some("missing_test_model_130".to_owned());
    crate::m1::detection::populate_detection_from_state(&mut state, &mut input);
    assert_eq!(
        input.detection_status,
        SensorStatus::DegradedSensorFailed {
            reason_code: error_codes::DETECTION_MODEL_NOT_LOADED.to_owned(),
        }
    );
}
