//! #255: inject observation data through the existing M1State fixture, but
//! drive the real request resolver and window-source decision. No OCR or
//! desktop focus changes are needed; these are not live/FSV acceptance tests.

use anyhow::{Context, ensure};
use synapse_core::{ElementId, OcrBackend, Rect, error_codes};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, ShowWindow, WS_EX_NOACTIVATE,
    WS_MINIMIZEBOX, WS_POPUP,
};
use windows::core::w;

use super::{M1State, ReadTextCaptureSource, ReadTextParams, resolve_read_text_request};
use crate::test_support::{PumpedWindows, from_hwnd, to_hwnd};

/// Root popup owned by a message-pumping UI thread, so UIA reads of it (and of
/// its #256 child) are answered while the test thread blocks in UIA calls.
struct Popup255 {
    ui: PumpedWindows,
}

impl Popup255 {
    fn open() -> anyhow::Result<Self> {
        // Mirror daemon startup (main.rs): the process is per-monitor-v2 aware, so
        // UIA element rects and window capture bounds share physical pixels. Without
        // it, results depend on whether an earlier test in this process set it.
        synapse_capture::init_process_dpi_awareness()
            .map_err(|error| anyhow::anyhow!("#255 init process DPI awareness: {error}"))?;
        let ui = PumpedWindows::spawn(|| {
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("Static"),
                    w!("synapse-255-focused-window"),
                    WS_POPUP | WS_MINIMIZEBOX,
                    100,
                    120,
                    320,
                    200,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .context("#255 CreateWindowExW")?;
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            Ok(vec![from_hwnd(hwnd)])
        })?;
        Ok(Self { ui })
    }

    fn hwnd(&self) -> i64 {
        self.ui.hwnd(0)
    }

    fn minimize(&self) -> anyhow::Result<()> {
        let hwnd = self.hwnd();
        self.ui.run(move || {
            let _ = unsafe { ShowWindow(to_hwnd(hwnd), SW_SHOWMINNOACTIVE) };
        })
    }
}

fn params255() -> ReadTextParams {
    ReadTextParams {
        region: None,
        element_id: None,
        window_hwnd: None,
        backend: OcrBackend::Auto,
        lang_hint: None,
    }
}

fn focused_state255(hwnd: i64, bbox: Rect) -> anyhow::Result<M1State> {
    let mut input = crate::m1::synthetic_notepad_input();
    let element_id = ElementId::parse(&format!("{hwnd:#x}:2a"))?;
    let focused = input.focused.as_mut().context("fixture focused element")?;
    focused.element_id = element_id.clone();
    focused.bbox = bbox;
    // Deliberately disagree: source must come from focused.element_id,
    // never the ambient foreground HWND. Keep a matching depth-zero node:
    // current_input's depth limiter otherwise replaces the focused element.
    input.foreground.hwnd = 0;
    input.elements.truncate(1);
    let node = input.elements.first_mut().context("fixture root node")?;
    node.element_id = element_id;
    node.bbox = bbox;
    node.focused = true;
    Ok(M1State {
        synthetic: Some(input),
        ..M1State::default()
    })
}

fn inset255(hwnd: i64) -> anyhow::Result<Rect> {
    let frame = synapse_capture::window_capture_bounds(hwnd)?;
    Ok(Rect {
        x: frame.x + 25,
        y: frame.y + 30,
        w: 80,
        h: 24,
    })
}

fn assert_refused255(state: &M1State, expected_code: &str) -> anyhow::Result<()> {
    let error = resolve_read_text_request(state, &params255(), None)
        .err()
        .context("#255: uncapturable focused window must refuse; Screen fallback would read another window")?;
    let actual = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str);
    ensure!(
        actual == Some(expected_code),
        "#255: expected typed refusal {expected_code}, got {error:?}"
    );
    Ok(())
}

#[test]
fn must_pass_255_no_arg_read_uses_focused_element_window_and_exact_crop() -> anyhow::Result<()> {
    let window = Popup255::open()?;
    let hwnd = window.hwnd();
    let bbox = inset255(hwnd)?;
    let state = focused_state255(hwnd, bbox)?;
    let resolved = resolve_read_text_request(&state, &params255(), None)
        .map_err(|error| anyhow::anyhow!("#255 no-arg resolution: {error:?}"))?;
    ensure!(resolved.region == bbox, "#255: focused screen bbox changed");
    ensure!(
        resolved.capture_source
            == ReadTextCaptureSource::Window {
                hwnd,
                window_region: Rect {
                    x: 25,
                    y: 30,
                    w: 80,
                    h: 24,
                },
            },
        "#255: no-arg read must capture focused element's own window, got {:?}",
        resolved.capture_source
    );
    Ok(())
}

#[test]
fn must_fail_255_no_arg_focused_element_without_hwnd_refuses_screen() -> anyhow::Result<()> {
    let state = focused_state255(
        0,
        Rect {
            x: 110,
            y: 150,
            w: 80,
            h: 24,
        },
    )?;
    assert_refused255(&state, error_codes::TARGET_WINDOW_NOT_FOUND)
}

#[test]
fn must_fail_255_no_arg_destroyed_focused_window_refuses_screen() -> anyhow::Result<()> {
    let window = Popup255::open()?;
    let state = focused_state255(window.hwnd(), inset255(window.hwnd())?)?;
    drop(window);
    assert_refused255(&state, error_codes::TARGET_WINDOW_NOT_FOUND)
}

#[test]
fn must_fail_255_no_arg_minimized_focused_window_refuses_screen() -> anyhow::Result<()> {
    let window = Popup255::open()?;
    let state = focused_state255(window.hwnd(), inset255(window.hwnd())?)?;
    window.minimize()?;
    assert_refused255(
        &state,
        error_codes::A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE,
    )
}

#[test]
fn must_fail_255_no_arg_focused_bbox_outside_own_window_refuses_screen() -> anyhow::Result<()> {
    let window = Popup255::open()?;
    let frame = synapse_capture::window_capture_bounds(window.hwnd())?;
    let state = focused_state255(
        window.hwnd(),
        Rect {
            x: frame.x + frame.w + 40,
            y: frame.y,
            w: 80,
            h: 24,
        },
    )?;
    assert_refused255(&state, error_codes::OCR_NO_TEXT)
}

#[test]
fn must_pass_255_explicit_region_keeps_screen_even_with_uncapturable_focus() -> anyhow::Result<()> {
    let bbox = Rect {
        x: 110,
        y: 150,
        w: 80,
        h: 24,
    };
    let state = focused_state255(0, bbox)?;
    let mut params = params255();
    params.region = Some(bbox);
    let resolved = resolve_read_text_request(&state, &params, None)
        .map_err(|error| anyhow::anyhow!("#255 explicit region: {error:?}"))?;
    ensure!(
        resolved.region == bbox && resolved.capture_source == ReadTextCaptureSource::Screen,
        "#255: explicit screen region must bypass focused-window resolution, got {resolved:?}"
    );
    Ok(())
}

#[test]
fn must_pass_255_explicit_element_keeps_178_window_route() -> anyhow::Result<()> {
    let window = Popup255::open()?;
    let hwnd = window.hwnd();
    let subtree = synapse_a11y::snapshot_window_from_hwnd(hwnd, 1)?;
    let mut params = params255();
    params.element_id = Some(subtree.root);
    let resolved = resolve_read_text_request(&M1State::default(), &params, None)
        .map_err(|error| anyhow::anyhow!("#255 explicit element: {error:?}"))?;
    ensure!(
        matches!(resolved.capture_source, ReadTextCaptureSource::Window { hwnd: actual, .. } if actual == hwnd),
        "#255: explicit element must retain #178 window capture, got {resolved:?}"
    );
    Ok(())
}

#[test]
fn must_pass_255_explicit_target_bypasses_uncapturable_focus() -> anyhow::Result<()> {
    let window = Popup255::open()?;
    let state = focused_state255(0, inset255(window.hwnd())?)?;
    for via_params in [false, true] {
        let mut params = params255();
        let target = if via_params {
            params.window_hwnd = Some(window.hwnd());
            None
        } else {
            Some(window.hwnd())
        };
        let resolved = resolve_read_text_request(&state, &params, target)
            .map_err(|error| anyhow::anyhow!("#255 explicit target: {error:?}"))?;
        ensure!(
            resolved.capture_source
                == ReadTextCaptureSource::WholeWindow {
                    hwnd: window.hwnd()
                },
            "#255: explicit target must bypass focus (via_params={via_params}), got {resolved:?}"
        );
    }
    Ok(())
}

// #256 reuses the #255 real-window fixture. Child coordinates deliberately
// differ from the root origin, catching normalization without crop rebasing.
// The child is created on the root's UI thread, so it is pumped too.
struct Child256(i64);

impl Child256 {
    fn open(parent: &Popup255) -> anyhow::Result<Self> {
        use windows::Win32::UI::WindowsAndMessaging::{WS_CHILD, WS_VISIBLE};
        let parent_hwnd = parent.hwnd();
        let hwnd = parent.ui.run(move || {
            let created = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("Edit"),
                    w!("synapse-256-child-text"),
                    WS_CHILD | WS_VISIBLE,
                    57,
                    43,
                    120,
                    36,
                    Some(to_hwnd(parent_hwnd)),
                    None,
                    None,
                    None,
                )
            };
            created.map(from_hwnd).map_err(|error| error.to_string())
        })?;
        let hwnd =
            hwnd.map_err(|error| anyhow::anyhow!("#256 CreateWindowExW child Edit: {error}"))?;
        Ok(Self(hwnd))
    }

    const fn hwnd(&self) -> i64 {
        self.0
    }

    fn element(&self) -> anyhow::Result<(ElementId, Rect)> {
        let subtree = synapse_a11y::snapshot_window_from_hwnd(self.hwnd(), 1)?;
        ensure!(
            subtree.root.parts()?.hwnd == self.hwnd(),
            "#256 fixture must exercise an actual child HWND element"
        );
        let bbox = synapse_a11y::element_bounding_rect(&subtree.root)?;
        ensure!(bbox.w > 0 && bbox.h > 0, "#256 child must be visible");
        Ok((subtree.root, bbox))
    }
}

// No Drop for Child256: destroying the root (PumpedWindows drop) destroys it.

fn assert_capture256(
    resolved: &super::ResolvedReadTextRequest,
    root_hwnd: i64,
    bbox: Rect,
) -> anyhow::Result<()> {
    let frame = synapse_capture::window_capture_bounds(root_hwnd)?;
    ensure!(
        resolved.region == bbox
            && resolved.capture_source
                == ReadTextCaptureSource::Window {
                    hwnd: root_hwnd,
                    window_region: Rect {
                        x: bbox.x - frame.x,
                        y: bbox.y - frame.y,
                        w: bbox.w,
                        h: bbox.h,
                    },
                },
        "#256 must capture root HWND and rebase child bbox to ROOT frame, got {resolved:?}"
    );
    Ok(())
}

#[test]
fn must_pass_256_child_element_id_uses_root_capture_and_root_relative_crop() -> anyhow::Result<()> {
    let root = Popup255::open()?;
    let child = Child256::open(&root)?;
    let (element_id, bbox) = child.element()?;
    let mut params = params255();
    params.element_id = Some(element_id);
    // No synthetic state: exercise #178 live UIA lookup inside text_region.
    let resolved = resolve_read_text_request(&M1State::default(), &params, None)
        .map_err(|error| anyhow::anyhow!("#256 visible child element must resolve: {error:?}"))?;
    assert_capture256(&resolved, root.hwnd(), bbox)
}

#[test]
fn must_pass_256_no_arg_child_focus_uses_root_capture_and_root_relative_crop() -> anyhow::Result<()>
{
    let root = Popup255::open()?;
    let child = Child256::open(&root)?;
    let (_, bbox) = child.element()?;
    let state = focused_state255(child.hwnd(), bbox)?;
    let resolved = resolve_read_text_request(&state, &params255(), None)
        .map_err(|error| anyhow::anyhow!("#256 focused child must resolve: {error:?}"))?;
    assert_capture256(&resolved, root.hwnd(), bbox)
}

#[test]
fn must_pass_256_top_level_element_and_focus_keep_same_hwnd_and_crop() -> anyhow::Result<()> {
    let root = Popup255::open()?;
    let subtree = synapse_a11y::snapshot_window_from_hwnd(root.hwnd(), 1)?;
    let bbox = synapse_a11y::element_bounding_rect(&subtree.root)?;
    let mut params = params255();
    params.element_id = Some(subtree.root);
    let explicit = resolve_read_text_request(&M1State::default(), &params, None)
        .map_err(|error| anyhow::anyhow!("#256 top-level element: {error:?}"))?;
    assert_capture256(&explicit, root.hwnd(), bbox)?;
    let bbox = inset255(root.hwnd())?;
    let state = focused_state255(root.hwnd(), bbox)?;
    let focused = resolve_read_text_request(&state, &params255(), None)
        .map_err(|error| anyhow::anyhow!("#256 top-level focus: {error:?}"))?;
    assert_capture256(&focused, root.hwnd(), bbox)
}

fn assert_child_refused256(hwnd: i64, bbox: Rect, code: &str) -> anyhow::Result<()> {
    let state = focused_state255(hwnd, bbox)?;
    assert_refused255(&state, code)?;
    // Also preserve #178's element-source contract independently of UIA
    // re-resolution, which may reject dead/minimized elements before capture.
    let id = ElementId::parse(&format!("{hwnd:#x}:2a"))?;
    let error = super::element_window_capture_source(&id, bbox)
        .err()
        .context("#256 unsafe child capture must refuse rather than read screen")?;
    let actual = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str);
    ensure!(actual == Some(code), "#256 expected {code}, got {error:?}");
    Ok(())
}

#[test]
fn must_fail_256_child_of_minimized_root_returns_typed_minimized_refusal() -> anyhow::Result<()> {
    let root = Popup255::open()?;
    let child = Child256::open(&root)?;
    let (_, bbox) = child.element()?;
    root.minimize()?;
    assert_child_refused256(
        child.hwnd(),
        bbox,
        error_codes::A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE,
    )
}

#[test]
fn must_fail_256_child_of_destroyed_root_returns_target_window_not_found() -> anyhow::Result<()> {
    let root = Popup255::open()?;
    let child = Child256::open(&root)?;
    let (_, bbox) = child.element()?;
    let hwnd = child.hwnd();
    // Parent destruction invalidates both handles; no new windows before readback.
    drop(root);
    assert_child_refused256(hwnd, bbox, error_codes::TARGET_WINDOW_NOT_FOUND)
}

#[test]
fn must_fail_256_child_bbox_outside_root_refuses_screen_fallback() -> anyhow::Result<()> {
    let root = Popup255::open()?;
    let child = Child256::open(&root)?;
    let frame = synapse_capture::window_capture_bounds(root.hwnd())?;
    assert_child_refused256(
        child.hwnd(),
        Rect {
            x: frame.x + frame.w + 40,
            y: frame.y,
            w: 80,
            h: 24,
        },
        error_codes::OCR_NO_TEXT,
    )
}

#[test]
fn must_pass_256_explicit_region_bypasses_child_with_minimized_root() -> anyhow::Result<()> {
    let root = Popup255::open()?;
    let child = Child256::open(&root)?;
    let (_, bbox) = child.element()?;
    let state = focused_state255(child.hwnd(), bbox)?;
    root.minimize()?;
    let mut params = params255();
    params.region = Some(bbox);
    let resolved = resolve_read_text_request(&state, &params, None)
        .map_err(|error| anyhow::anyhow!("#256 explicit screen region: {error:?}"))?;
    ensure!(
        resolved.region == bbox && resolved.capture_source == ReadTextCaptureSource::Screen,
        "#256 explicit region must retain exact screen capture: {resolved:?}"
    );
    Ok(())
}
