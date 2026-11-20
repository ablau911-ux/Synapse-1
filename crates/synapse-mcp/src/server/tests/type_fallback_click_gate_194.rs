//! #194: the act_type Chromium foreground fallback runs the actionability gate
//! (scroll into view, live bbox refresh, click point inside the target window)
//! BEFORE its real-cursor click, and a refusal sends no mouse input at all.
//! The existing `click_actionability_194` tests cover only the pure
//! `visible_click_point`; deleting the caller's gate left them green. These
//! drive the fallback click step with an injected gate and a recording action
//! backend (no real cursor).
//!
//! NEEDS-SEAM (fixer extracts from `act_type` lines "#194: same actionability
//! gate ..." + `click_act_type_foreground_fallback_target`, behaviour unchanged):
//!   async fn act_type_fallback_actionable_click(
//!       handle: ActionHandle,
//!       element_id: &ElementId,
//!       target: &ActTypeForegroundFallbackTarget,
//!       actionable: impl FnOnce(&ElementId, i64, &[UiaPattern], &mut Rect)
//!           -> Result<Point, foreground_click_actionability::ClickPointRefusal>,
//!   ) -> Result<ActTypeForegroundFallbackTarget, ErrorData>
//! `act_type` calls it with `foreground_click_actionability::actionable_click_point`.

use std::cell::Cell;
use std::sync::Arc;

use synapse_action::{ActionBackend, ActionEmitter, RecordedInput, RecordingBackend};
use synapse_core::{ElementId, MouseTarget, Point, Rect, UiaPattern, error_codes};
use tokio_util::sync::CancellationToken;

use super::foreground_click_actionability::ClickPointRefusal;
use super::*;

const ROOT: i64 = 0x0042_0000;
const STALE: Rect = Rect {
    x: 200,
    y: 1500,
    w: 100,
    h: 40,
};

fn target() -> ActTypeForegroundFallbackTarget {
    ActTypeForegroundFallbackTarget {
        element_id: "0x420000:2a00000007".to_owned(),
        root_hwnd: ROOT,
        process_name: "chrome.exe".to_owned(),
        role: "edit".to_owned(),
        automation_id_present: false,
        bbox: STALE,
        enabled: true,
        keyboard_focusable: true,
        patterns: vec![UiaPattern::Value, UiaPattern::ScrollItem],
        name_len: 5,
        value_len: Some(0),
    }
}

fn element() -> ElementId {
    ElementId::parse("0x420000:2a00000007").expect("synthetic element id")
}

fn recorder() -> (Arc<RecordingBackend>, ActionHandle, CancellationToken) {
    let recording = Arc::new(RecordingBackend::new());
    let backend: Arc<dyn ActionBackend> = Arc::clone(&recording) as Arc<dyn ActionBackend>;
    let cancel = CancellationToken::new();
    let (handle, _snapshots, _join) = ActionEmitter::spawn_with_backend(cancel.clone(), backend);
    (recording, handle, cancel)
}

fn mouse_events(events: &[RecordedInput]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RecordedInput::MouseMove { .. }
                    | RecordedInput::MouseMoveAbsolute { .. }
                    | RecordedInput::MouseButtonDown { .. }
                    | RecordedInput::MouseButtonUp { .. }
            )
        })
        .count()
}

#[tokio::test]
async fn must_fail_point_outside_the_window_is_refused_with_zero_mouse_input() {
    let (recording, handle, cancel) = recorder();
    let gate_calls = Cell::new(0);
    let error = act_type_fallback_actionable_click(
        handle,
        &element(),
        &target(),
        |id, root, patterns, bbox| {
            gate_calls.set(gate_calls.get() + 1);
            assert_eq!((id, root), (&element(), ROOT));
            assert!(patterns.contains(&UiaPattern::ScrollItem));
            *bbox = STALE; // live re-read: still below the fold
            Err(ClickPointRefusal {
                code: error_codes::ACTION_TARGET_INVALID,
                reason: "target_click_point_outside_window",
                detail: "click point (250, 1520) is outside the target window".to_owned(),
            })
        },
    )
    .await
    .expect_err("an off-viewport target must not be clicked");
    cancel.cancel();
    assert_eq!(gate_calls.get(), 1);
    let data = error.data.as_ref().expect("structured refusal");
    assert_eq!(data["code"], error_codes::ACTION_TARGET_INVALID);
    assert_eq!(data["reason"], "target_click_point_outside_window");
    assert_eq!(data["tool"], "act_type");
    assert_eq!(mouse_events(&recording.events()), 0, "no stray click");
}

#[tokio::test]
async fn must_fail_bbox_refresh_failure_is_refused_with_zero_mouse_input() {
    let (recording, handle, cancel) = recorder();
    let error = act_type_fallback_actionable_click(handle, &element(), &target(), |_, _, _, _| {
        Err(ClickPointRefusal {
            code: error_codes::A11Y_ELEMENT_STALE,
            reason: "bbox_refresh_failed",
            detail: "live bounding-rect readback failed".to_owned(),
        })
    })
    .await
    .expect_err("an unreadable live bbox must not be clicked");
    cancel.cancel();
    let data = error.data.as_ref().expect("structured refusal");
    assert_eq!(data["code"], error_codes::A11Y_ELEMENT_STALE);
    assert_eq!(data["reason"], "bbox_refresh_failed");
    assert_eq!(mouse_events(&recording.events()), 0);
}

#[tokio::test]
async fn must_pass_scrolled_target_is_clicked_once_at_its_refreshed_centre() {
    let (recording, handle, cancel) = recorder();
    let refreshed = Rect {
        x: 200,
        y: 300,
        w: 100,
        h: 40,
    };
    let clicked =
        act_type_fallback_actionable_click(handle, &element(), &target(), |_, _, _, bbox| {
            *bbox = refreshed;
            Ok(Point { x: 250, y: 320 })
        })
        .await
        .expect("visible target is clicked");
    cancel.cancel();
    assert_eq!(
        clicked.bbox, refreshed,
        "the click uses the live bbox, not the stale one"
    );
    let events = recording.events();
    let moves: Vec<Point> = events
        .iter()
        .filter_map(|event| match event {
            RecordedInput::MouseMove {
                to: MouseTarget::Screen { point },
                ..
            }
            | RecordedInput::MouseMoveAbsolute { point } => Some(*point),
            _ => None,
        })
        .collect();
    assert_eq!(moves.first(), Some(&Point { x: 250, y: 320 }), "{events:?}");
    let downs = events
        .iter()
        .filter(|event| matches!(event, RecordedInput::MouseButtonDown { .. }))
        .count();
    assert_eq!(downs, 1, "exactly one click: {events:?}");
}

/// Wiring: act_type's fallback uses the gated click with the live gate.
#[test]
fn must_pass_act_type_fallback_uses_the_gated_click() {
    let source = include_str!("../m2_tools.rs");
    let start = source.find("pub async fn act_type(").expect("handler");
    let body = &source[start..];
    let body = &body[..body.find("\n    #[tool(").unwrap_or(body.len())];
    assert!(
        body.contains("act_type_fallback_actionable_click("),
        "{}",
        body.len()
    );
    assert!(body.contains("foreground_click_actionability::actionable_click_point"));
}
