//! #232: End-to-end tests for sparse empty-container windows across live visible
//! and minimized `window_input_from_hwnd`, `hidden_desktop_input_from_worker_snapshot`,
//! and `settle_cdp_snapshot_status` -> `ObservationAssembler::assemble` + `match_find_input`.

use anyhow::Context;
use synapse_core::{AccessibleSubtree, PerceptionMode, Rect, SensorStatus};
use synapse_perception::{ObservationAssembler, ObservationInput, ObserveInclude};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, HMENU, RegisterClassW, SW_SHOWMINNOACTIVE,
            SW_SHOWNOACTIVATE, ShowWindow, WINDOW_EX_STYLE, WM_CLOSE, WNDCLASSW, WS_CHILD,
            WS_MINIMIZEBOX, WS_POPUP, WS_VISIBLE,
        },
    },
    core::w,
};

use super::*;
use crate::m1::{FindParams, match_find_input, settle_cdp_snapshot_status};
use crate::test_support::{PumpedWindows, from_hwnd};

/// Root window whose only child is an unnamed, empty pane, owned by a
/// message-pumping UI thread (`PumpedWindows`).
///
/// The root is a frameless `WS_POPUP`: a captioned `WS_OVERLAPPEDWINDOW`
/// exposes a raw title-bar subtree (title bar -> system menu bar -> system
/// menu item) whose system menu bar sits at depth 2 with a child, so the
/// depth-2 walk is legitimately truncated (#77) and the sparse-content verdict
/// under test is never reached (same finding as `walker_streaming_61`).
struct SparseHostWindow {
    hwnd: i64,
    _ui: PumpedWindows,
}

impl SparseHostWindow {
    fn spawn(minimized: bool) -> Self {
        let ui = PumpedWindows::spawn(move || create_sparse_windows(minimized))
            .expect("sparse host window ready");
        Self {
            hwnd: ui.hwnd(0),
            _ui: ui,
        }
    }
}

fn create_sparse_windows(minimized: bool) -> anyhow::Result<Vec<i64>> {
    unsafe {
        let instance = GetModuleHandleW(None).context("module handle")?;
        let class_name = w!("SynapseSparsePane232Window");
        let class = WNDCLASSW {
            lpfnWndProc: Some(sparse_wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&raw const class);
        let root = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("SparsePane232Root"),
            WS_POPUP | WS_MINIMIZEBOX,
            160,
            160,
            360,
            240,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .context("create root window")?;
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!(""),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            320,
            200,
            Some(root),
            Some(HMENU(701 as *mut _)),
            Some(instance.into()),
            None,
        )
        .context("create empty child pane")?;
        let show_cmd = if minimized {
            SW_SHOWMINNOACTIVE
        } else {
            SW_SHOWNOACTIVATE
        };
        let _ = ShowWindow(root, show_cmd);
        Ok(vec![from_hwnd(root)])
    }
}

unsafe extern "system" fn sparse_wnd_proc(
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
fn must_fail_live_visible_window_with_only_empty_child_pane_degrades_observe_and_find() {
    let win = SparseHostWindow::spawn(false);
    let input = window_input_from_hwnd(win.hwnd, 2, PerceptionMode::A11yOnly)
        .expect("window_input_from_hwnd visible sparse window");
    let expected = SensorStatus::DegradedSensorFailed {
        reason_code: A11Y_TARGET_WINDOW_NO_UIA_CONTENT.to_owned(),
    };
    assert_eq!(input.a11y_status, expected);

    let obs = ObservationAssembler::new()
        .assemble(ObserveInclude::default(), input.clone())
        .expect("assemble observation");
    assert_eq!(obs.diagnostics.a11y_status, expected);
    assert_eq!(
        match_find_input(&input, &FindParams::default())
            .a11y_coverage
            .as_deref(),
        Some(A11Y_TARGET_WINDOW_NO_UIA_CONTENT)
    );
}

#[test]
fn must_fail_live_minimized_sparse_window_and_hidden_desktop_sparse_snapshot_degrade_observe_and_find()
 {
    let win = SparseHostWindow::spawn(true);
    let min_input = window_input_from_hwnd(win.hwnd, 2, PerceptionMode::A11yOnly)
        .expect("window_input_from_hwnd minimized sparse window");
    assert!(min_input.is_minimized);
    assert_eq!(
        min_input.a11y_status,
        SensorStatus::DegradedSensorFailed {
            reason_code: A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE.to_owned(),
        }
    );
    assert_eq!(
        match_find_input(&min_input, &FindParams::default())
            .a11y_coverage
            .as_deref(),
        Some(A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE)
    );

    let mut root = node(0, 0, "Host", "window", false);
    root.children_count = 1;
    let tree = AccessibleSubtree {
        root: root.element_id.clone(),
        nodes: vec![root, node(1, 1, "", "pane", false)],
        max_depth: 2,
        truncated: false,
        selected_content_root: None,
        completeness_scope: None,
        stop_reason: None,
        skipped_subtree_roots: Vec::new(),
    };
    let hd_input = hidden_desktop_input_from_worker_snapshot(
        tree,
        foreground_with_bounds(Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        }),
        PerceptionMode::A11yOnly,
    );
    assert_eq!(
        hd_input.a11y_status,
        SensorStatus::DegradedSensorFailed {
            reason_code: A11Y_TARGET_WINDOW_NO_UIA_CONTENT.to_owned(),
        }
    );
}

#[test]
fn must_fail_blocked_or_empty_cdp_snapshot_does_not_supersede_no_uia_content() {
    for (returned, blocked_frames, frame_errors) in [(0, 0, 0), (15, 1, 0), (15, 0, 1)] {
        let mut input = ObservationInput::new(foreground_with_bounds(Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        }));
        input.a11y_status = SensorStatus::DegradedSensorFailed {
            reason_code: A11Y_TARGET_WINDOW_NO_UIA_CONTENT.to_owned(),
        };
        settle_cdp_snapshot_status(&mut input, returned, 20, 300, blocked_frames, frame_errors);
        assert_eq!(
            input.a11y_status,
            SensorStatus::DegradedSensorFailed {
                reason_code: A11Y_TARGET_WINDOW_NO_UIA_CONTENT.to_owned(),
            },
            "case=({returned}, {blocked_frames}, {frame_errors}) must not clear sparse UIA status"
        );
    }
}

#[test]
fn must_pass_unblocked_non_empty_cdp_snapshot_supersedes_no_uia_content_to_healthy() {
    let mut input = ObservationInput::new(foreground_with_bounds(Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    }));
    input.elements = vec![
        node(0, 0, "Spotify", "window", false),
        node(1, 1, "Play", "button", false),
    ];
    input.a11y_status = SensorStatus::DegradedSensorFailed {
        reason_code: A11Y_TARGET_WINDOW_NO_UIA_CONTENT.to_owned(),
    };
    input.capture_status = SensorStatus::Disabled;

    settle_cdp_snapshot_status(&mut input, 18, 25, 300, 0, 0);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
    assert_eq!(
        match_find_input(&input, &FindParams::default()).a11y_coverage,
        None
    );
}
