//! #61: Streaming `raw_walker` child enumeration in `collect_nodes` /
//! `collect_child_nodes` must stop at `node_budget` or `deadline` without bulk
//! materializing remaining siblings, and flag `truncated = true`.

use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use uiautomation::types::{ElementMode, Handle};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HMENU, MSG,
            PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow,
            TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WNDCLASSW, WS_CHILD, WS_POPUP, WS_VISIBLE,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use super::{
    super::common::{TreeView, create_cache_request, map_uia_error, with_automation},
    SnapshotWalk, collect_nodes,
};

struct MultiChildTestWindow {
    hwnd: i64,
    stop_tx: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

impl MultiChildTestWindow {
    fn spawn(child_count: usize) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let join = thread::spawn(move || unsafe {
            let instance = GetModuleHandleW(None).expect("module handle");
            let class_name = w!("SynapseWalkerStreaming61Window");
            let class = WNDCLASSW {
                lpfnWndProc: Some(walker_wnd_proc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            let _ = RegisterClassW(&raw const class);
            let root = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("WalkerStreaming61Root"),
                // Frameless: a title bar's raw-view subtree (system menu bar ->
                // menu item) sits below depth 2 and would legitimately mark the
                // depth-2 walk truncated (#77), masking the #61 budget contract.
                WS_POPUP,
                160,
                160,
                400,
                320,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .expect("create root window");
            for idx in 0..child_count {
                let label = HSTRING::from(format!("StreamChild{idx}"));
                let _ = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    PCWSTR(label.as_ptr()),
                    WS_CHILD | WS_VISIBLE,
                    12,
                    12 + i32::try_from(idx).unwrap_or(0) * 34,
                    140,
                    28,
                    Some(root),
                    Some(HMENU((600 + idx) as *mut _)),
                    Some(instance.into()),
                    None,
                )
                .expect("create child button");
            }
            let _ = ShowWindow(root, SW_SHOWNOACTIVATE);
            let _ = ready_tx.send(root.0 as isize as i64);
            let mut msg = MSG::default();
            loop {
                while PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).into() {
                    let _ = TranslateMessage(&raw const msg);
                    DispatchMessageW(&raw const msg);
                }
                if stop_rx.recv_timeout(Duration::from_millis(10)).is_ok() {
                    let _ = DestroyWindow(root);
                    break;
                }
            }
        });
        let hwnd = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("multi-child window ready");
        Self {
            hwnd,
            stop_tx,
            join: Some(join),
        }
    }
}

impl Drop for MultiChildTestWindow {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

unsafe extern "system" fn walker_wnd_proc(
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

fn walk_window_with_budget(
    hwnd: i64,
    node_budget: usize,
) -> (Vec<synapse_core::AccessibleNode>, bool) {
    with_automation(move |automation| {
        let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Raw)?;
        let true_condition = automation.create_true_condition().map_err(map_uia_error)?;
        let raw_walker = automation.get_raw_view_walker().map_err(map_uia_error)?;
        let root_raw = isize::try_from(hwnd).expect("valid hwnd");
        let cached_root = automation
            .element_from_handle_build_cache(Handle::from(root_raw), &cache)
            .map_err(map_uia_error)?;
        let mut nodes = Vec::new();
        let mut truncated = false;
        let walk = SnapshotWalk {
            true_condition: &true_condition,
            cache: &cache,
            raw_walker: &raw_walker,
            root_hwnd: hwnd,
            node_budget,
            deadline: Instant::now() + Duration::from_secs(5),
        };
        collect_nodes(&walk, &cached_root, None, 0, 2, &mut nodes, &mut truncated)?;
        Ok((nodes, truncated))
    })
    .expect("walk_window_with_budget")
}

#[test]
fn must_fail_streaming_child_walker_halts_at_node_budget_and_flags_truncated() {
    let win = MultiChildTestWindow::spawn(6);
    let (nodes, truncated) = walk_window_with_budget(win.hwnd, 3);
    assert!(
        truncated,
        "streaming walk hitting node_budget=3 must set truncated"
    );
    assert_eq!(
        nodes.len(),
        3,
        "streaming walker must stop at node_budget=3 without collecting remaining siblings"
    );
}

#[test]
fn must_pass_streaming_child_walker_collects_all_children_when_under_budget() {
    let win = MultiChildTestWindow::spawn(4);
    let (nodes, truncated) = walk_window_with_budget(win.hwnd, 64);
    assert!(!truncated, "walk under budget must not set truncated");
    for idx in 0..4 {
        let expected = format!("StreamChild{idx}");
        assert!(
            nodes.iter().any(|n| n.name == expected),
            "missing {expected} in {nodes:?}"
        );
    }
}
