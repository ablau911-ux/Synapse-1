//! #101: End-to-end provider-to-consumer regression tests proving offscreen UIA
//! child controls (`IsOffscreen == true`), anonymous/zero-area nodes, and
//! partial-stem noise are excluded from `find` and `observe(interactable_only=true)`.

use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use synapse_core::PerceptionMode;
use synapse_perception::{ObservationAssembler, ObserveInclude};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HMENU, MSG,
            PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow,
            TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN,
            WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    },
    core::w,
};

use super::*;
use crate::m1::{FindParams, match_find_input};

struct OffscreenTestWindow {
    hwnd: i64,
    stop_tx: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

impl OffscreenTestWindow {
    fn spawn() -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let join = thread::spawn(move || unsafe {
            let instance = GetModuleHandleW(None).expect("module handle");
            let class_name = w!("SynapseOffscreen101Window");
            let class = WNDCLASSW {
                lpfnWndProc: Some(offscreen_wnd_proc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            let _ = RegisterClassW(&raw const class);
            let root = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("Offscreen101Root"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                180,
                180,
                360,
                240,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .expect("create root window");
            let _visible_save = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!("Save Document"),
                WS_CHILD | WS_VISIBLE,
                16,
                16,
                140,
                36,
                Some(root),
                Some(HMENU(401 as *mut _)),
                Some(instance.into()),
                None,
            )
            .expect("create visible Save Document button");
            let _visible_stem_noise = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!("Saved Documented"),
                WS_CHILD | WS_VISIBLE,
                16,
                64,
                140,
                36,
                Some(root),
                Some(HMENU(402 as *mut _)),
                Some(instance.into()),
                None,
            )
            .expect("create visible stem-noise button");
            let _hidden_save = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!("Save Hidden Offscreen"),
                WS_CHILD,
                16,
                112,
                140,
                36,
                Some(root),
                Some(HMENU(403 as *mut _)),
                Some(instance.into()),
                None,
            )
            .expect("create hidden button");
            let _clipped_save = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!("Save Clipped Offscreen"),
                WS_CHILD | WS_VISIBLE,
                2400,
                2400,
                140,
                36,
                Some(root),
                Some(HMENU(404 as *mut _)),
                Some(instance.into()),
                None,
            )
            .expect("create clipped offscreen button");
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
            .expect("offscreen test window ready");
        Self {
            hwnd,
            stop_tx,
            join: Some(join),
        }
    }
}

impl Drop for OffscreenTestWindow {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

unsafe extern "system" fn offscreen_wnd_proc(
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
fn must_fail_live_offscreen_controls_have_zero_visibility_bbox_and_never_appear_in_find_or_interactable_observe()
 {
    let win = OffscreenTestWindow::spawn();
    let input = window_input_from_hwnd(win.hwnd, 2, PerceptionMode::A11yOnly)
        .expect("window_input_from_hwnd");

    for node in input
        .elements
        .iter()
        .filter(|n| n.name == "Save Hidden Offscreen" || n.name == "Save Clipped Offscreen")
    {
        assert_eq!(
            (node.bbox.w, node.bbox.h),
            (0, 0),
            "offscreen control {:?} must have zero visibility extent",
            node.name
        );
    }

    let find = match_find_input(
        &input,
        &FindParams {
            query: Some("Save".to_owned()),
            ..Default::default()
        },
    );
    let names: Vec<_> = find
        .results
        .iter()
        .filter_map(|r| r.name.as_deref())
        .collect();
    assert!(names.contains(&"Save Document"), "got: {names:?}");
    assert!(!names.contains(&"Save Hidden Offscreen"), "got: {names:?}");
    assert!(!names.contains(&"Save Clipped Offscreen"), "got: {names:?}");

    let include = ObserveInclude {
        interactable_only: true,
        ..ObserveInclude::default()
    };
    let obs = ObservationAssembler::new()
        .assemble(include, input)
        .expect("assemble interactable_only");
    assert!(obs.elements.iter().any(|n| n.name == "Save Document"));
    assert!(
        !obs.elements
            .iter()
            .any(|n| n.name == "Save Hidden Offscreen")
    );
    assert!(
        !obs.elements
            .iter()
            .any(|n| n.name == "Save Clipped Offscreen")
    );
}

#[test]
fn must_fail_find_query_rejects_partial_word_stems_and_anonymous_or_zero_area_elements() {
    let win = OffscreenTestWindow::spawn();
    let mut input = window_input_from_hwnd(win.hwnd, 2, PerceptionMode::A11yOnly)
        .expect("window_input_from_hwnd");
    let mut anonymous = node(90, 1, "   ", "Button", false);
    anonymous.automation_id = None;
    let mut zero_area = node(91, 1, "Save Document Zero", "Button", false);
    zero_area.bbox.w = 0;
    input.elements.push(anonymous);
    input.elements.push(zero_area);

    let find = match_find_input(
        &input,
        &FindParams {
            query: Some("save document".to_owned()),
            ..Default::default()
        },
    );
    let names: Vec<_> = find
        .results
        .iter()
        .filter_map(|r| r.name.as_deref())
        .collect();
    assert_eq!(names, vec!["Save Document"]);
}

#[test]
fn must_pass_visible_matching_control_and_root_window_geometry_survive_provider_snapshot() {
    let win = OffscreenTestWindow::spawn();
    let input = window_input_from_hwnd(win.hwnd, 2, PerceptionMode::A11yOnly)
        .expect("window_input_from_hwnd");
    let root = input.elements.first().expect("root window node");
    assert_eq!(root.depth, 0);
    assert!(root.bbox.w > 0 && root.bbox.h > 0);

    let save_doc = input
        .elements
        .iter()
        .find(|n| n.name == "Save Document")
        .expect("Save Document node present");
    assert!(save_doc.bbox.w > 0 && save_doc.bbox.h > 0);
}
