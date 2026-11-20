//! #194: injected native tier behavior for both real caller flag combinations.
//! The actionability double models ScrollItem + bbox refresh; it does not
//! execute or verify the OS implementation of actionable_click_point.
use super::*;
use std::cell::RefCell;
use synapse_core::{ElementId, Rect};

const WINDOW: Rect = Rect {
    x: 100,
    y: 200,
    w: 400,
    h: 300,
};
const VISIBLE: Rect = Rect {
    x: 200,
    y: 300,
    w: 100,
    h: 40,
};
const STALE: Rect = Rect {
    x: 200,
    y: 1500,
    w: 100,
    h: 40,
};

// Real caller wiring is pinned by the separate source-guard tests in
// element_gate_variants_194.rs, so a reformat or move fails only those guards
// and never masks this behavioural evidence.
fn probe(
    recording: bool,
    foreground_coordinate: bool,
    initial: Rect,
    refreshed: Rect,
) -> (Result<Point, ErrorData>, Vec<&'static str>) {
    let id = synapse_core::element_id(194, "01020304");
    let events = RefCell::new(Vec::new());
    let read_bbox = |actual: &ElementId| {
        assert_eq!(actual, &id);
        events.borrow_mut().push("bbox");
        Ok(initial)
    };
    let result = native_click_point_with(
        &id,
        194,
        recording,
        foreground_coordinate,
        read_bbox,
        |root| {
            assert_eq!(root, 194);
            events.borrow_mut().push("bounds");
            Ok(WINDOW)
        },
        |actual| {
            assert_eq!(actual, &id);
            events.borrow_mut().push("metadata");
            Ok(synapse_a11y::ElementMetadataReadback {
                name: "scroll target".to_owned(),
                role: "Button".to_owned(),
                automation_id: None,
                bbox: initial,
                enabled: true,
                keyboard_focusable: true,
                patterns: vec![UiaPattern::ScrollItem],
                value: None,
            })
        },
        |actual, root, patterns, bbox| {
            assert_eq!(actual, &id);
            assert_eq!(root, 194);
            assert_eq!(patterns, &[UiaPattern::ScrollItem]);
            assert_eq!(*bbox, initial);
            events.borrow_mut().push("scroll");
            // Separate injected readback after scrolling: never reuse metadata bbox.
            events.borrow_mut().push("reread");
            *bbox = refreshed;
            events.borrow_mut().push("check");
            crate::server::foreground_click_actionability::visible_click_point(*bbox, WINDOW)
        },
    );
    (result, events.into_inner())
}

fn scrolls(events: &[&str]) -> usize {
    events.iter().filter(|event| **event == "scroll").count()
}

#[test]
fn must_pass_background_postmessage_has_zero_scroll_calls() -> anyhow::Result<()> {
    let (result, events) = probe(false, false, VISIBLE, STALE);
    assert_eq!(result?, Point { x: 250, y: 320 });
    assert_eq!(scrolls(&events), 0, "#194: PostMessage must never scroll");
    assert_eq!(events, ["bbox", "bounds"]);
    Ok(())
}

#[test]
fn must_pass_recording_backend_has_zero_scroll_calls() -> anyhow::Result<()> {
    for foreground_coordinate in [false, true] {
        let (result, events) = probe(true, foreground_coordinate, STALE, VISIBLE);
        assert_eq!(result?, Point { x: 250, y: 1520 });
        assert_eq!(scrolls(&events), 0, "#194: recording must never scroll");
        assert_eq!(events, ["bbox"], "recording must skip live window reads");
    }
    Ok(())
}

#[test]
fn must_pass_foreground_coordinate_scrolls_once_then_rereads_and_checks() -> anyhow::Result<()> {
    let (result, events) = probe(false, true, STALE, VISIBLE);
    assert_eq!(
        scrolls(&events),
        1,
        "#194: foreground must invoke scroll once"
    );
    assert_eq!(events, ["metadata", "scroll", "reread", "check"]);
    assert_eq!(result?, Point { x: 250, y: 320 }, "must use refreshed bbox");
    Ok(())
}

#[test]
fn must_fail_foreground_refreshed_empty_outside_and_boundary_bbox() {
    for (refreshed, reason) in [
        (Rect { w: 0, ..VISIBLE }, "target_bbox_empty"),
        (STALE, "target_click_point_outside_window"),
        (
            Rect {
                x: 490,
                w: 20,
                ..VISIBLE
            },
            "target_click_point_outside_window",
        ),
    ] {
        let (result, events) = probe(false, true, VISIBLE, refreshed);
        assert_eq!(scrolls(&events), 1);
        assert_eq!(events, ["metadata", "scroll", "reread", "check"]);
        let Err(error) = result else {
            panic!("#194: refreshed invalid bbox must refuse")
        };
        let Some(data) = error.data else {
            panic!("structured refusal required")
        };
        assert_eq!(data["code"], "ACTION_TARGET_INVALID");
        assert_eq!(data["reason"], reason);
        assert_eq!(
            data["element_id"],
            synapse_core::element_id(194, "01020304").to_string()
        );
    }
}

#[test]
fn must_fail_background_outside_bbox_without_attempting_scroll() {
    let (result, events) = probe(false, false, STALE, VISIBLE);
    assert_eq!(scrolls(&events), 0);
    assert_eq!(events, ["bbox", "bounds"]);
    let Err(error) = result else {
        panic!("background outside window must refuse")
    };
    let Some(data) = error.data else {
        panic!("structured refusal required")
    };
    assert_eq!(data["reason"], "target_click_point_outside_window");
}
