//! Geometry-epoch registry tests (#130): the per-window move/resize
//! generation counter must tick on real window motion — including
//! move-and-return, where before/after bounds compare equal but the window
//! was not geometrically still. These tests create real top-level windows
//! and move them with `SetWindowPos`, so they exercise the same WinEvent
//! path the capture daemon relies on.

#![cfg(windows)]

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use synapse_capture::{geometry_epoch, geometry_epoch_quiescent, geometry_epoch_status};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, RegisterClassW,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, ShowWindow,
    WINDOW_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::w;

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

static WINDOW_CLASS: OnceLock<()> = OnceLock::new();

struct TestWindow {
    hwnd: HWND,
}

impl TestWindow {
    fn new(x: i32, y: i32) -> Self {
        WINDOW_CLASS.get_or_init(|| unsafe {
            let module = match GetModuleHandleW(None) {
                Ok(module) => module,
                Err(error) => panic!("GetModuleHandleW failed: {error}"),
            };
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: w!("synapse_geometry_epoch_test_wnd"),
                hInstance: module.into(),
                ..WNDCLASSW::default()
            };
            let atom = RegisterClassW(&class);
            if atom == 0 {
                panic!("RegisterClassW failed for the geometry-epoch test window");
            }
        });
        let hwnd = match unsafe {
            CreateWindowExW(
                Default::default(),
                w!("synapse_geometry_epoch_test_wnd"),
                w!("synapse geometry epoch test"),
                WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
                x,
                y,
                320,
                200,
                None,
                None,
                None,
                None,
            )
        } {
            Ok(hwnd) => hwnd,
            Err(error) => panic!("CreateWindowExW failed: {error}"),
        };
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        Self { hwnd }
    }

    fn id(&self) -> i64 {
        self.hwnd.0 as i64
    }

    fn rect(&self) -> (i32, i32, i32, i32) {
        let mut rect = RECT::default();
        match unsafe { GetWindowRect(self.hwnd, &raw mut rect) } {
            Ok(()) => (rect.left, rect.top, rect.right, rect.bottom),
            Err(error) => panic!("GetWindowRect failed: {error}"),
        }
    }

    fn move_by(&self, dx: i32, dy: i32) {
        let (left, top, _, _) = self.rect();
        match unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                left + dx,
                top + dy,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        } {
            Ok(()) => {}
            Err(error) => panic!("SetWindowPos failed: {error}"),
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Polls `predicate` every 5 ms until it holds or `timeout` elapses.
fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    predicate()
}

/// The registry installs its hook and a real window move advances the
/// per-window epoch plus the global change counter.
#[test]
fn must_pass_registry_is_active_and_ticks_on_real_window_move() {
    let active = wait_until(Duration::from_secs(2), || geometry_epoch_status().active);
    assert!(
        active,
        "the WinEvent hook must install on an interactive desktop"
    );
    let status_before = geometry_epoch_status();

    let window = TestWindow::new(120, 120);
    let epoch_before = geometry_epoch_quiescent(window.id());
    window.move_by(37, 23);

    let ticked = wait_until(Duration::from_secs(2), || {
        geometry_epoch(window.id()) > epoch_before
    });
    assert!(
        ticked,
        "moving a real window must advance its geometry epoch (was {epoch_before})"
    );
    let status_after = geometry_epoch_status();
    assert!(
        status_after.total_changes > status_before.total_changes,
        "the global change counter must advance: {} -> {}",
        status_before.total_changes,
        status_after.total_changes
    );
}

/// The #130 regression: a window that moves away and returns has equal
/// before/after bounds, yet its epoch advanced — bounds equality alone is
/// not geometric stability.
#[test]
fn must_fail_move_and_return_advances_epoch_though_bounds_are_equal() {
    let window = TestWindow::new(240, 160);
    let epoch_stable = geometry_epoch_quiescent(window.id());
    let rect_before = window.rect();

    window.move_by(48, 0);
    let ticked_away = wait_until(Duration::from_secs(2), || {
        geometry_epoch(window.id()) > epoch_stable
    });
    assert!(ticked_away, "the move away must tick the epoch");
    let epoch_away = geometry_epoch(window.id());

    window.move_by(-48, 0);
    let ticked_back = wait_until(Duration::from_secs(2), || {
        geometry_epoch(window.id()) != epoch_away
    });
    assert!(ticked_back, "the move back must tick the epoch again");

    assert_eq!(
        window.rect(),
        rect_before,
        "precondition: the window returned to its exact original bounds"
    );
    assert!(
        geometry_epoch_quiescent(window.id()) > epoch_stable,
        "move-and-return must advance the epoch even though bounds compare equal"
    );
}

/// An untouched window's epoch must not change while time passes.
#[test]
fn must_pass_still_window_epoch_is_stable() {
    let window = TestWindow::new(360, 200);
    let first = geometry_epoch_quiescent(window.id());
    std::thread::sleep(Duration::from_millis(120));
    let second = geometry_epoch_quiescent(window.id());
    assert_eq!(
        first, second,
        "an untouched window's geometry epoch must not change"
    );
}

/// Per-window epochs are independent: moving one window must not tick
/// another's epoch.
#[test]
fn must_fail_moving_one_window_does_not_tick_another() {
    let mover = TestWindow::new(480, 120);
    let bystander = TestWindow::new(480, 360);
    let epoch_bystander = geometry_epoch_quiescent(bystander.id());
    let epoch_mover = geometry_epoch_quiescent(mover.id());

    mover.move_by(25, 25);
    let ticked = wait_until(Duration::from_secs(2), || {
        geometry_epoch(mover.id()) > epoch_mover
    });
    assert!(ticked, "the moved window's epoch must advance");

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        geometry_epoch_quiescent(bystander.id()),
        epoch_bystander,
        "moving one window must not tick another window's epoch"
    );
}
