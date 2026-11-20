//! #261 regression guard: starting monitor WGC capture after a thread that owned
//! a Win32 window has exited must not crash the process (a STATUS_ACCESS_VIOLATION
//! was seen 2026-10-16 02:40 with an earlier livefeed test fixture). Each case
//! runs 5 start/stop cycles in one process; a crash fails the whole test binary.
//! Not reproducible on the 2026-10-16 tree with these three fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(windows)]

#[path = "../../synapse-mcp/tests/g6a_window/mod.rs"]
mod g6a_window;

use std::time::Duration;

use g6a_window::{BLUE, PaintedWindow, WindowSpec};
use synapse_capture::{CaptureConfig, CaptureTarget, spawn_capture_loop};

fn cap_cycle(tag: &str, i: usize) {
    let handle = spawn_capture_loop(CaptureConfig {
        target: CaptureTarget::Primary,
        ..CaptureConfig::default().with_env_backend()
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    handle.stop().unwrap();
    eprintln!("{tag}{i} ok");
}

#[test]
fn must_pass_261_capture_while_window_alive_then_window_thread_exits() {
    for i in 0..5 {
        let window = PaintedWindow::open(WindowSpec::visible(200, 120, BLUE));
        cap_cycle("D", i);
        drop(window);
    }
}

#[test]
fn must_pass_261_window_thread_exits_before_each_capture_start() {
    for i in 0..5 {
        drop(PaintedWindow::open(WindowSpec::visible(200, 120, BLUE)));
        cap_cycle("H", i);
    }
}

mod exact {
    use std::sync::OnceLock;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
                PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage,
                WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP,
            },
        },
    };
    use windows::core::w;

    unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    pub fn window_thread_cycle() {
        std::thread::spawn(|| unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            static CLASS: OnceLock<()> = OnceLock::new();
            let module = GetModuleHandleW(None).unwrap();
            CLASS.get_or_init(|| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: module.into(),
                    lpszClassName: w!("synapse_capture_261_window"),
                    ..WNDCLASSW::default()
                };
                assert_ne!(RegisterClassW(&raw const class), 0);
            });
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                w!("synapse_capture_261_window"),
                w!("synapse-capture-261"),
                WS_POPUP,
                40,
                40,
                160,
                100,
                None,
                None,
                Some(module.into()),
                None,
            )
            .unwrap();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let mut msg = MSG::default();
            for _ in 0..30 {
                while PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&raw const msg);
                    DispatchMessageW(&raw const msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            DestroyWindow(hwnd).unwrap();
        })
        .join()
        .unwrap();
    }
}

#[test]
fn must_pass_261_topmost_noactivate_dpi_aware_window_thread_exit_then_capture() {
    for i in 0..5 {
        exact::window_thread_cycle();
        cap_cycle("X", i);
    }
}
