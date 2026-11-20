//! Window-lifetime generation tests (#130): destroying a top-level window must
//! advance that HWND value's generation (and its geometry epoch, so an
//! in-flight capture rejects), because Windows recycles HWND values and
//! per-HWND state would otherwise leak from a destroyed window to its successor.

#![cfg(windows)]

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use synapse_capture::{geometry_epoch_quiescent, geometry_epoch_status, window_generation};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, ShowWindow, WINDOW_STYLE, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
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

fn create_window() -> HWND {
    WINDOW_CLASS.get_or_init(|| unsafe {
        let module = match GetModuleHandleW(None) {
            Ok(module) => module,
            Err(error) => panic!("GetModuleHandleW failed: {error}"),
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            lpszClassName: w!("synapse_window_generation_test_wnd"),
            hInstance: module.into(),
            ..WNDCLASSW::default()
        };
        if RegisterClassW(&class) == 0 {
            panic!("RegisterClassW failed for the window-generation test window");
        }
    });
    let hwnd = match unsafe {
        CreateWindowExW(
            Default::default(),
            w!("synapse_window_generation_test_wnd"),
            w!("synapse window generation test"),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
            140,
            140,
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
    hwnd
}

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

/// Destroying a window advances its generation and epoch; a sibling window's
/// generation is untouched.
#[test]
fn must_pass_destroying_a_window_advances_its_generation_and_epoch_only() {
    assert!(
        wait_until(Duration::from_secs(2), || geometry_epoch_status().active),
        "the WinEvent hook must install on an interactive desktop"
    );
    let victim = create_window();
    let sibling = create_window();
    let victim_id = victim.0 as i64;
    let sibling_id = sibling.0 as i64;

    let victim_generation_before = window_generation(victim_id);
    let victim_epoch_before = geometry_epoch_quiescent(victim_id);
    let sibling_generation_before = window_generation(sibling_id);

    if let Err(error) = unsafe { DestroyWindow(victim) } {
        panic!("DestroyWindow failed: {error}");
    }

    let advanced = wait_until(Duration::from_secs(2), || {
        window_generation(victim_id) > victim_generation_before
    });
    assert!(advanced, "destroy must advance the HWND's generation");
    assert!(
        geometry_epoch_quiescent(victim_id) > victim_epoch_before,
        "destroy must also advance the geometry epoch so in-flight captures reject"
    );
    assert_eq!(
        window_generation(sibling_id),
        sibling_generation_before,
        "destroying one window must not touch another window's generation"
    );
    let _ = unsafe { DestroyWindow(sibling) };
}

/// Moving a live window is geometry motion, not a new window lifetime: the
/// geometry epoch ticks but the HWND's generation must not, or per-window
/// state of a still-living window would be discarded as a recycled handle.
#[test]
fn must_fail_moving_a_live_window_never_advances_its_generation() {
    assert!(
        wait_until(Duration::from_secs(2), || geometry_epoch_status().active),
        "the WinEvent hook must install on an interactive desktop"
    );
    let window = create_window();
    let window_id = window.0 as i64;

    let generation_before = window_generation(window_id);
    let epoch_before = geometry_epoch_quiescent(window_id);

    if let Err(error) = unsafe {
        SetWindowPos(
            window,
            None,
            180,
            170,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    } {
        panic!("SetWindowPos failed: {error}");
    }

    let moved = wait_until(Duration::from_secs(2), || {
        geometry_epoch_quiescent(window_id) > epoch_before
    });
    assert!(moved, "the move must be observed by the geometry epoch");
    assert_eq!(
        window_generation(window_id),
        generation_before,
        "a move of a live window must not advance its lifetime generation"
    );
    let _ = unsafe { DestroyWindow(window) };
}
