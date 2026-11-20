//! #77: End-to-end producer-to-consumer truncation propagation across live Win32
//! UIA depth cuts, Chromium renderer UIA supplements, and CDP node-budget cuts
//! into both `ObservationAssembler::assemble` and `match_find_input`.

use anyhow::Context;
use synapse_core::{
    CdpDiagnostics, CdpStatus, PerceptionMode, Rect, SensorStatus, WebPerceptionPath,
};
use synapse_perception::{ObservationAssembler, ObservationInput, ObserveInclude};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, HMENU, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow,
            WINDOW_EX_STYLE, WM_CLOSE, WNDCLASSW, WS_CHILD, WS_POPUP, WS_VISIBLE,
        },
    },
    core::w,
};

use super::*;
use crate::m1::{FindParams, match_find_input, settle_cdp_snapshot_status};
use crate::test_support::{PumpedWindows, from_hwnd};

/// Root -> container -> button window owned by a message-pumping UI thread
/// (`PumpedWindows`), so every UIA message is answered at once and the
/// 400 ms snapshot deadline measures the walk, not a polling pump.
///
/// The root is a frameless `WS_POPUP`: a captioned `WS_OVERLAPPEDWINDOW`
/// adds a raw title-bar subtree (title bar -> system menu bar -> system menu
/// item) that is deeper than the 3-level tree this fixture models, so a depth
/// cut inside it legitimately flags the snapshot truncated (#77) and hides the
/// contract under test (same finding as `walker_streaming_61`).
struct NestedTestWindow {
    hwnd: i64,
    _ui: PumpedWindows,
}

impl NestedTestWindow {
    fn spawn() -> Self {
        let ui = PumpedWindows::spawn(create_nested_windows).expect("nested test window ready");
        Self {
            hwnd: ui.hwnd(0),
            _ui: ui,
        }
    }
}

fn create_nested_windows() -> anyhow::Result<Vec<i64>> {
    unsafe {
        let instance = GetModuleHandleW(None).context("module handle")?;
        let class_name = w!("SynapseTruncation77Window");
        let class = WNDCLASSW {
            lpfnWndProc: Some(nested_wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&raw const class);
        let root = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("Truncation77Root"),
            WS_POPUP,
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
        let panel = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("ContainerPanel"),
            WS_CHILD | WS_VISIBLE,
            12,
            12,
            280,
            160,
            Some(root),
            Some(HMENU(301 as *mut _)),
            Some(instance.into()),
            None,
        )
        .context("create child container")?;
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            w!("DeepAction"),
            WS_CHILD | WS_VISIBLE,
            10,
            10,
            140,
            36,
            Some(panel),
            Some(HMENU(302 as *mut _)),
            Some(instance.into()),
            None,
        )
        .context("create grandchild button")?;
        let _ = ShowWindow(root, SW_SHOWNOACTIVATE);
        Ok(vec![from_hwnd(root)])
    }
}

unsafe extern "system" fn nested_wnd_proc(
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

fn truncated_sensor_status() -> SensorStatus {
    SensorStatus::DegradedSensorFailed {
        reason_code: A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED.to_owned(),
    }
}

#[test]
fn must_fail_depth_cut_on_live_nested_window_propagates_truncation_to_observe_and_find() {
    let win = NestedTestWindow::spawn();
    let input = window_input_from_hwnd(win.hwnd, 1, PerceptionMode::A11yOnly)
        .expect("window_input_from_hwnd at depth 1");
    assert_eq!(input.a11y_status, truncated_sensor_status());

    let observation = ObservationAssembler::new()
        .assemble(ObserveInclude::default(), input.clone())
        .expect("assemble observation");
    assert_eq!(
        observation.diagnostics.a11y_status,
        truncated_sensor_status()
    );

    let find = match_find_input(&input, &FindParams::default());
    assert_eq!(
        find.a11y_coverage.as_deref(),
        Some(A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED)
    );
}

#[test]
fn must_pass_full_depth_on_live_nested_window_reports_healthy_observe_and_find() {
    let win = NestedTestWindow::spawn();
    let input = window_input_from_hwnd(win.hwnd, 3, PerceptionMode::A11yOnly)
        .expect("window_input_from_hwnd at depth 3");
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
    assert!(
        input.elements.iter().any(|n| n.name == "DeepAction"),
        "expected grandchild button at depth 3, got: {:?}",
        input.elements
    );

    let observation = ObservationAssembler::new()
        .assemble(ObserveInclude::default(), input.clone())
        .expect("assemble observation");
    assert_eq!(observation.diagnostics.a11y_status, SensorStatus::Healthy);

    let find = match_find_input(&input, &FindParams::default());
    assert_eq!(find.a11y_coverage, None);
}

#[test]
fn must_fail_truncated_chromium_renderer_supplement_propagates_to_observe_and_find() {
    let mut fg = foreground_with_bounds(Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    });
    fg.process_name = "msedge.exe".to_owned();
    let mut input = ObservationInput::new(fg);
    input.elements = vec![node(0, 0, "Edge", "Window", false)];
    input.a11y_status = SensorStatus::Healthy;
    input.capture_status = SensorStatus::Disabled;
    input.web_path = Some(WebPerceptionPath::UiaOnly);
    input.cdp = Some(CdpDiagnostics {
        process_name: "msedge.exe".to_owned(),
        status: CdpStatus::Unreachable,
        endpoint: None,
        checked_ports: vec![9222],
        checked_endpoints: Vec::new(),
        reason_code: Some("A11Y_CDP_UNREACHABLE".to_owned()),
        detail: None,
        capabilities: Vec::new(),
        attached_node_count: None,
        selected_target_id: None,
        selected_session_id: None,
        target_selection_reason: None,
        target_candidate_count: None,
        frame_tree_frame_count: None,
        attached_frame_target_count: None,
        blocked_frame_targets: Vec::new(),
        frame_snapshot_errors: Vec::new(),
    });

    let supplement_btn = node(9, 2, "RendererSubmit", "Button", false);
    supplement_chromium_renderer_accessibility_with(&mut input, 2, |_, _, _| {
        Some((vec![supplement_btn], true))
    });

    let observation = ObservationAssembler::new()
        .assemble(ObserveInclude::default(), input.clone())
        .expect("assemble observation");
    assert_eq!(
        observation.diagnostics.a11y_status,
        truncated_sensor_status()
    );
    assert!(
        observation
            .elements
            .iter()
            .any(|n| n.name == "RendererSubmit")
    );
    assert_eq!(
        match_find_input(&input, &FindParams::default())
            .a11y_coverage
            .as_deref(),
        Some(A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED)
    );
}

#[test]
fn must_fail_cdp_budget_truncation_after_superseding_sparse_uia_propagates_to_observe_and_find() {
    let mut input = ObservationInput::new(foreground_with_bounds(Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    }));
    input.elements = vec![
        node(0, 0, "App", "Window", false),
        node(1, 1, "Save", "button", false),
    ];
    input.a11y_status = SensorStatus::DegradedSensorFailed {
        reason_code: A11Y_TARGET_WINDOW_NO_UIA_CONTENT.to_owned(),
    };
    input.capture_status = SensorStatus::Disabled;

    settle_cdp_snapshot_status(&mut input, 300, 750, 300, 0, 0);

    let observation = ObservationAssembler::new()
        .assemble(ObserveInclude::default(), input.clone())
        .expect("assemble observation");
    assert_eq!(
        observation.diagnostics.a11y_status,
        truncated_sensor_status()
    );
    assert_eq!(
        match_find_input(&input, &FindParams::default())
            .a11y_coverage
            .as_deref(),
        Some(A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED)
    );
}
