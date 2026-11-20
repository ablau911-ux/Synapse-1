//! #124: Caller-level regression tests proving `snapshot_from_root` and
//! `snapshot_element` classify a destroyed subtree root as `A11Y_ELEMENT_STALE`
//! (including when the control disappears between resolution and cache refresh).

use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use synapse_core::{ElementId, error_codes::A11Y_ELEMENT_STALE};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HMENU, MSG,
            PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow,
            TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW,
            WS_VISIBLE,
        },
    },
    core::w,
};

use super::{
    super::{
        common::with_automation,
        resolve::{element_metadata, re_resolve_on_worker},
    },
    invalidate_snapshot_cache, snapshot_element, snapshot_from_root, snapshot_window_from_hwnd,
};
use crate::A11yError;

enum WindowCmd {
    DestroyChild(mpsc::SyncSender<()>),
    Stop,
}

struct DestroyableChildWindow {
    root_hwnd: i64,
    cmd_tx: mpsc::Sender<WindowCmd>,
    join: Option<JoinHandle<()>>,
}

impl DestroyableChildWindow {
    fn spawn() -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let join = thread::spawn(move || unsafe {
            let instance = GetModuleHandleW(None).expect("module handle");
            let class_name = w!("SynapseStaleSubtree124Window");
            let class = WNDCLASSW {
                lpfnWndProc: Some(stale_wnd_proc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            let _ = RegisterClassW(&raw const class);
            let root = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("StaleSubtree124Root"),
                WS_OVERLAPPEDWINDOW,
                160,
                160,
                340,
                220,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .expect("create root window");
            let mut child = Some(
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    w!("EphemeralBranch"),
                    WS_CHILD | WS_VISIBLE,
                    16,
                    16,
                    160,
                    40,
                    Some(root),
                    Some(HMENU(501 as *mut _)),
                    Some(instance.into()),
                    None,
                )
                .expect("create child button"),
            );
            let _ = ShowWindow(root, SW_SHOWNOACTIVATE);
            let _ = ready_tx.send(root.0 as isize as i64);
            let mut msg = MSG::default();
            loop {
                while PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).into() {
                    let _ = TranslateMessage(&raw const msg);
                    DispatchMessageW(&raw const msg);
                }
                match cmd_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(WindowCmd::DestroyChild(ack)) => {
                        if let Some(hwnd) = child.take() {
                            let _ = DestroyWindow(hwnd);
                        }
                        let _ = ack.send(());
                    }
                    Ok(WindowCmd::Stop) => {
                        let _ = DestroyWindow(root);
                        break;
                    }
                    Err(_) => {}
                }
            }
        });
        let root_hwnd = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("destroyable child window ready");
        Self {
            root_hwnd,
            cmd_tx,
            join: Some(join),
        }
    }

    fn find_child_id(&self) -> ElementId {
        invalidate_snapshot_cache();
        let tree = snapshot_window_from_hwnd(self.root_hwnd, 2).expect("snapshot window");
        tree.nodes
            .into_iter()
            .find(|node| node.name == "EphemeralBranch")
            .expect("EphemeralBranch present")
            .element_id
    }

    fn destroy_child(&self) {
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(WindowCmd::DestroyChild(ack_tx))
            .expect("send DestroyChild");
        ack_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("child destroyed");
        invalidate_snapshot_cache();
    }
}

impl Drop for DestroyableChildWindow {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(WindowCmd::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

unsafe extern "system" fn stale_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLOSE {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[test]
fn must_pass_live_subtree_root_refreshes_cleanly() {
    let win = DestroyableChildWindow::spawn();
    let child_id = win.find_child_id();
    let subtree = snapshot_element(&child_id, 2).expect("live snapshot_element succeeds");
    assert_eq!(subtree.root, child_id);
    assert!(subtree.nodes.iter().any(|n| n.name == "EphemeralBranch"));
}

#[test]
fn must_fail_subtree_root_destroyed_between_resolve_and_cache_refresh_returns_element_stale() {
    let win = DestroyableChildWindow::spawn();
    let child_id = win.find_child_id();
    let (destroy_req_tx, destroy_req_rx) = mpsc::sync_channel(1);
    let (destroy_done_tx, destroy_done_rx) = mpsc::sync_channel(1);

    let worker_handle = thread::spawn(move || {
        with_automation(move |automation| {
            let resolved = re_resolve_on_worker(automation, &child_id)?;
            let _ = destroy_req_tx.send(());
            let _ = destroy_done_rx.recv_timeout(Duration::from_secs(5));
            snapshot_from_root(automation, &resolved, 2)
        })
    });

    // The job may queue behind other tests holding the shared UIA worker (e.g.
    // the #257 long-walk tests hold its exclusive test gate for seconds).
    destroy_req_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("resolved on worker before destroy");
    win.destroy_child();
    let _ = destroy_done_tx.send(());

    let err = worker_handle
        .join()
        .expect("worker join")
        .expect_err("destroyed UIElement must fail snapshot_from_root");
    assert!(
        matches!(err, A11yError::ElementStale { .. }),
        "expected ElementStale on race after resolve, got: {err:?}"
    );
    assert_eq!(err.code(), A11Y_ELEMENT_STALE);
}

#[test]
fn must_fail_snapshot_element_and_metadata_after_child_destroyed_return_element_stale() {
    let win = DestroyableChildWindow::spawn();
    let child_id = win.find_child_id();
    win.destroy_child();

    let snap_err = snapshot_element(&child_id, 2)
        .expect_err("snapshot_element on destroyed control must fail");
    assert!(
        matches!(snap_err, A11yError::ElementStale { .. }),
        "expected ElementStale, got: {snap_err:?}"
    );
    assert_eq!(snap_err.code(), A11Y_ELEMENT_STALE);

    let meta_err =
        element_metadata(&child_id).expect_err("element_metadata on destroyed control must fail");
    assert_eq!(meta_err.code(), A11Y_ELEMENT_STALE);
}
