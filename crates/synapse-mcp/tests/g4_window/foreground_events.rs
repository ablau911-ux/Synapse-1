//! Passive OS event journal for #107; catches activate-then-restore that an
//! after-only GetForegroundWindow assertion misses. Never activates a window.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{cell::RefCell, sync::mpsc, thread::JoinHandle, time::Duration};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
        WindowsAndMessaging::{
            DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW,
            PostThreadMessageW, WINEVENT_OUTOFCONTEXT, WM_QUIT,
        },
    },
};

thread_local! { static EVENTS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) }; }

unsafe extern "system" fn record(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        EVENTS.with(|events| events.borrow_mut().push(hwnd.0 as i64));
    }
}

pub struct ForegroundEvents {
    thread_id: u32,
    worker: Option<JoinHandle<Vec<i64>>>,
}

impl ForegroundEvents {
    pub fn start() -> Self {
        let (ready, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut msg = MSG::default();
            // SAFETY: creates this thread's own message queue, then installs a
            // passive out-of-context hook; callbacks execute on this thread.
            unsafe {
                let _ = PeekMessageW(&raw mut msg, None, 0, 0, PM_NOREMOVE);
            }
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(record),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            assert!(!hook.0.is_null(), "foreground event hook installed");
            ready
                .send(unsafe { GetCurrentThreadId() })
                .expect("hook ready");
            while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.0 > 0 {
                unsafe {
                    DispatchMessageW(&raw const msg);
                }
            }
            assert!(
                unsafe { UnhookWinEvent(hook) }.as_bool(),
                "remove own passive hook"
            );
            EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
        });
        Self {
            thread_id: receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("hook thread ready"),
            worker: Some(worker),
        }
    }

    pub fn finish(mut self) -> Vec<i64> {
        self.stop()
    }

    fn stop(&mut self) -> Vec<i64> {
        let Some(worker) = self.worker.take() else {
            return Vec::new();
        };
        // SAFETY: this id is the hook thread, whose queue was created before ready.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
            .expect("stop foreground observer");
        worker.join().expect("foreground observer completed")
    }
}

impl Drop for ForegroundEvents {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
