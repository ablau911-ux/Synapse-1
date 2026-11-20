//! Test windows that live on their own message-pumping UI thread.
//!
//! UI Automation reaches a Win32 window through messages sent to the window's
//! owner thread (`WM_GETOBJECT`, `WM_GETTEXT`, ...). A window created on a
//! libtest thread that then blocks inside a UIA call never answers, so the
//! shared UIA worker times out and is retired, and every concurrent UIA test
//! fails with `phase=worker_unavailable` (T1 raid cascade, first hang #178).
//!
//! `PumpedWindows` creates the windows on a dedicated thread that pumps until
//! the fixture is dropped (drop destroys the windows on that thread). Anything
//! that must run on the owner thread (creating a child window, `ShowWindow`)
//! goes through [`PumpedWindows::run`].
//!
//! Tests that inspect messages posted to their window use
//! [`PumpedWindows::spawn_recording`]: the pump records those message ids
//! instead of dispatching them, and [`PumpedWindows::take_recorded`] reads them
//! back in post order. [`PerMonitorDpi`] keeps a thread's Win32 geometry in
//! physical pixels without changing the process DPI mode.

use std::cell::RefCell;

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetMessageW, IsWindow, MSG, PostThreadMessageW,
    TranslateMessage, WM_APP, WM_QUIT,
};

/// Thread message that tells the UI thread to drain its job queue.
const RUN_JOBS: u32 = WM_APP + 0x255;
const WAIT: Duration = Duration::from_secs(10);

type UiJob = Box<dyn FnOnce() + Send + 'static>;
type Created = Result<(u32, Vec<i64>), String>;
/// `(hwnd, message, wParam, lParam)` of a recorded posted message.
type Recorded = (i64, u32, usize, isize);

thread_local! {
    /// Messages the UI thread recorded instead of dispatching.
    static RECORDED: RefCell<Vec<Recorded>> = const { RefCell::new(Vec::new()) };
}

/// Windows owned by one pumping UI thread; destroyed when dropped.
pub(crate) struct PumpedWindows {
    thread_id: u32,
    hwnds: Vec<i64>,
    jobs: mpsc::Sender<UiJob>,
    join: Option<JoinHandle<()>>,
}

impl PumpedWindows {
    /// Runs `create` on a new UI thread; it returns the HWNDs it created.
    pub(crate) fn spawn(
        create: impl FnOnce() -> anyhow::Result<Vec<i64>> + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::spawn_recording(create, &[])
    }

    /// Like [`Self::spawn`]; posted messages whose id is in `recorded` are
    /// kept for [`Self::take_recorded`] instead of being dispatched.
    pub(crate) fn spawn_recording(
        create: impl FnOnce() -> anyhow::Result<Vec<i64>> + Send + 'static,
        recorded: &'static [u32],
    ) -> anyhow::Result<Self> {
        let (created_tx, created_rx) = mpsc::channel::<Created>();
        let (jobs, job_rx) = mpsc::channel::<UiJob>();
        let join = std::thread::Builder::new()
            .name("synapse-test-pumped-windows".to_owned())
            .spawn(move || ui_thread(create, &created_tx, &job_rx, recorded))
            .context("spawn pumped test window thread")?;
        let (thread_id, hwnds) = created_rx
            .recv_timeout(WAIT)
            .context("pumped test windows were not created in time")?
            .map_err(|error| anyhow::anyhow!("create pumped test windows: {error}"))?;
        Ok(Self {
            thread_id,
            hwnds,
            jobs,
            join: Some(join),
        })
    }

    /// HWND `index` as returned by the `create` closure.
    pub(crate) fn hwnd(&self, index: usize) -> i64 {
        self.hwnds.get(index).copied().unwrap_or(0)
    }

    /// Runs `job` on the UI thread and waits for its result.
    pub(crate) fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> anyhow::Result<T> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.jobs
            .send(Box::new(move || {
                let _sent = reply_tx.send(job());
            }))
            .map_err(|_| anyhow::anyhow!("pumped test window thread is gone"))?;
        // SAFETY: the UI thread owns a message queue (it created windows).
        unsafe { PostThreadMessageW(self.thread_id, RUN_JOBS, WPARAM(0), LPARAM(0)) }
            .context("post job to pumped test window thread")?;
        reply_rx
            .recv_timeout(WAIT)
            .context("pumped test window job did not finish in time")
    }

    /// Removes and returns `(wParam, lParam)` of every recorded `message`
    /// posted to `hwnd` so far, in post order. The read runs on the UI thread
    /// behind a thread message posted after the caller's earlier posts, so
    /// every message the caller already posted has been recorded.
    pub(crate) fn take_recorded(
        &self,
        hwnd: i64,
        message: u32,
    ) -> anyhow::Result<Vec<(usize, isize)>> {
        self.run(move || {
            RECORDED.with(|recorded| {
                let mut recorded = recorded.borrow_mut();
                let mut taken = Vec::new();
                recorded.retain(|&(target, id, wparam, lparam)| {
                    let hit = target == hwnd && id == message;
                    if hit {
                        taken.push((wparam, lparam));
                    }
                    !hit
                });
                taken
            })
        })
    }
}

/// Per-monitor-v2 DPI context for the calling thread until dropped. Windows
/// created under it, and geometry read under it, are in physical pixels.
pub(crate) struct PerMonitorDpi(DPI_AWARENESS_CONTEXT);

impl PerMonitorDpi {
    pub(crate) fn enter() -> Self {
        // SAFETY: thread-local DPI context switch; restored on drop.
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for PerMonitorDpi {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: restores the context this guard replaced.
            let _restored = unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

impl Drop for PumpedWindows {
    fn drop(&mut self) {
        // SAFETY: the UI thread owns a message queue (it created windows).
        let _posted = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _joined = join.join();
        }
    }
}

/// Converts a stored HWND value back to a handle.
pub(crate) fn to_hwnd(hwnd: i64) -> HWND {
    HWND(hwnd as isize as *mut core::ffi::c_void)
}

/// Converts a handle to the `i64` form Synapse APIs use.
pub(crate) fn from_hwnd(hwnd: HWND) -> i64 {
    hwnd.0 as isize as i64
}

fn ui_thread(
    create: impl FnOnce() -> anyhow::Result<Vec<i64>>,
    created: &mpsc::Sender<Created>,
    jobs: &mpsc::Receiver<UiJob>,
    recorded: &[u32],
) {
    let hwnds = match create() {
        Ok(hwnds) => hwnds,
        Err(error) => {
            let _sent = created.send(Err(format!("{error:#}")));
            return;
        }
    };
    // SAFETY: plain thread-id query.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _sent = created.send(Ok((thread_id, hwnds.clone())));
    pump(jobs, recorded);
    for hwnd in hwnds {
        // SAFETY: owner thread; skip handles already destroyed by the test.
        if unsafe { IsWindow(Some(to_hwnd(hwnd))) }.as_bool() {
            let _destroyed = unsafe { DestroyWindow(to_hwnd(hwnd)) };
        }
    }
}

fn pump(jobs: &mpsc::Receiver<UiJob>, recorded: &[u32]) {
    let mut msg = MSG::default();
    // SAFETY: standard message loop over this thread's own queue.
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.0 > 0 {
        if msg.hwnd.0.is_null() && msg.message == RUN_JOBS {
            while let Ok(job) = jobs.try_recv() {
                job();
            }
            continue;
        }
        if !msg.hwnd.0.is_null() && recorded.contains(&msg.message) {
            let record = (from_hwnd(msg.hwnd), msg.message, msg.wParam.0, msg.lParam.0);
            RECORDED.with(|messages| messages.borrow_mut().push(record));
            continue;
        }
        // SAFETY: dispatching a message this thread just received.
        unsafe {
            let _translated = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}
