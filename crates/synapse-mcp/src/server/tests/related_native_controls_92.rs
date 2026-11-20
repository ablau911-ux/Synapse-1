//! Owned native controls, pumped on a dedicated thread. The checkbox is the
//! active window of that thread only (thread-local activation, never the OS
//! foreground): a Win32 button ignores BM_CLICK - which UIA Toggle sends - while
//! its window is inactive, so a never-active fixture can never be toggled.
use std::{sync::mpsc, thread::JoinHandle, time::Duration};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::Input::KeyboardAndMouse::SetActiveWindow,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindowTextW, MSG,
            PostThreadMessageW, SendMessageW, TranslateMessage, WINDOW_STYLE, WM_QUIT,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
        },
    },
    core::w,
};

pub(super) struct Controls {
    pub(super) checkbox: i64,
    pub(super) edit: i64,
    thread_id: u32,
    join: Option<JoinHandle<()>>,
    // Topmost visible windows: hold the desktop-topology guard while they
    // exist; declared last so it is released after Drop destroyed them.
    _desktop: std::sync::MutexGuard<'static, ()>,
}

fn hwnd(value: i64) -> HWND {
    HWND(value as *mut core::ffi::c_void)
}

impl Controls {
    pub(super) fn spawn() -> Self {
        let desktop = crate::test_support::desktop_topology_serial();
        let (tx, rx) = mpsc::channel();
        let join = std::thread::spawn(move || {
            // SAFETY: all HWNDs are created, pumped and destroyed by this thread.
            unsafe {
                let checkbox = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    w!("BUTTON"),
                    w!("TF18 checkbox"),
                    WS_POPUP | WS_VISIBLE | WINDOW_STYLE(3), // BS_AUTOCHECKBOX
                    96,
                    96,
                    280,
                    80,
                    None,
                    None,
                    None,
                    None,
                )
                .expect("create owned checkbox");
                let edit = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    w!("EDIT"),
                    w!("before"),
                    WS_POPUP | WS_VISIBLE,
                    96,
                    200,
                    280,
                    80,
                    None,
                    None,
                    None,
                    None,
                )
                .expect("create owned edit");
                // Background thread: activation stays local to this thread's input
                // queue and does not change the foreground window.
                let _ = SetActiveWindow(checkbox);
                tx.send((checkbox.0 as i64, edit.0 as i64, GetCurrentThreadId()))
                    .expect("send fixture HWNDs");
                let mut msg = MSG::default();
                while GetMessageW(&raw mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&raw const msg);
                    DispatchMessageW(&raw const msg);
                }
                let _ = DestroyWindow(edit);
                let _ = DestroyWindow(checkbox);
            }
        });
        let (checkbox, edit, thread_id) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("owned controls ready");
        Self {
            checkbox,
            edit,
            thread_id,
            join: Some(join),
            _desktop: desktop,
        }
    }

    pub(super) fn checked(&self) -> bool {
        // SAFETY: owned HWND stays live until Drop; BM_GETCHECK takes no pointers.
        unsafe {
            SendMessageW(
                hwnd(self.checkbox),
                0x00F0,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            )
            .0 == 1
        }
    }

    pub(super) fn text(&self) -> String {
        let mut buffer = [0u16; 128];
        // SAFETY: owned HWND and stack buffer remain live throughout the read.
        let length = unsafe { GetWindowTextW(hwnd(self.edit), &mut buffer) };
        String::from_utf16_lossy(
            &buffer[..usize::try_from(length).expect("nonnegative text length")],
        )
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        // SAFETY: only our fixture thread receives WM_QUIT.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
