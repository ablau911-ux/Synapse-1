//! #194: injected point-gate behavior and production tier wiring.
//! Supporting source guards and OCR dispatch checks. Native injected behavior
//! lives in native_click_scroll_counts_194.rs; none of these prove live scrolling.
use super::*;
use std::cell::Cell;
use synapse_core::{ElementId, Rect};

/// The guarded code lives in element_coordinate.rs (tier callers) and
/// element_native.rs (point owner and gate). Comments and whitespace are dropped
/// and trailing commas folded so rustfmt line wrapping cannot change a binding.
fn compact_production_section(start: &str, end: &str) -> String {
    concat!(
        include_str!("element_coordinate.rs"),
        include_str!("element_native.rs")
    )
    .split_once(start)
    .expect("production owner exists")
    .1
    .split_once(end)
    .expect("production owner boundary exists")
    .0
    .lines()
    .map(|line| line.split("//").next().unwrap_or_default())
    .flat_map(str::chars)
    .filter(|ch| !ch.is_whitespace())
    .collect::<String>()
    .replace(",)", ")")
}

#[test]
fn must_pass_foreground_coordinate_tier_forwards_recording_and_enables_scroll_gate() {
    let native = compact_production_section(
        "impl CoordinateClickPort for LiveCoordinateClick",
        "async fn post(",
    );
    assert!(
        native.contains(
            "element_center(&self.element.element_id,self.recording.is_some(),foreground)"
        )
    );
    let caller = compact_production_section(
        "async fn coordinate_foreground_with(",
        "fn foreground_failure(",
    );
    assert!(
        caller.contains("port.point(true)"),
        "#194: only foreground resolves with scroll enabled"
    );
    assert!(
        caller.find("port.authorize()").expect("authorization gate")
            < caller
                .find("port.point(true)")
                .expect("foreground point gate"),
        "#194: permission precedes actionability scrolling"
    );
}

#[test]
fn must_pass_background_coordinate_tier_resolves_without_scroll_gate() {
    let caller = compact_production_section(
        "pub(crate) async fn coordinate_click_with(",
        "async fn coordinate_foreground_with(",
    );
    assert!(
        caller.contains("port.point(false)"),
        "#194: background PostMessage point must not enable the scroll gate"
    );
    assert!(!caller.contains("port.point(true)"));
}

#[test]
fn must_pass_element_center_routes_only_live_foreground_native_targets_to_scroll_and_check() {
    let owner = compact_production_section("fn element_center(", "fn element_center_with(");
    assert!(
        owner.contains("element_center_with(element_id,recording,"),
        "#194: real owner must forward recording to the tested point gate"
    );
    let (passive, foreground) = owner
        .split_once("letmetadata=read_metadata(element_id)?;")
        .expect("native foreground gate remains after passive branch");
    assert!(
        passive.contains("ifrecording||!foreground_coordinate{"),
        "#194: background and recording must take the passive branch"
    );
    assert!(
        passive.contains("letwindow_bounds=ifrecording{rect}else{read_bounds(root)?}"),
        "#194: only recording may bypass real HWND bounds"
    );
    assert!(passive.contains("returncrate::server::foreground_click_actionability::visible_click_point(rect,window_bounds)"),
        "#194: passive branch must return after visible-point check without scrolling");
    assert!(!passive.contains("actionable_click_point("));
    assert!(!passive.contains("scroll_element_into_view("));
    assert!(
        passive
            .contains("native_click_point_with(element_id,root,recording,foreground_coordinate,"),
        "#194: real owner must forward both tier flags into the injected native gate"
    );
    assert!(
        foreground.contains("actionable(element_id,root,&metadata.patterns,&mutbbox)"),
        "#194: live foreground native target must scroll, refresh and check the point"
    );
}

fn ocr(rect: Rect) -> ElementId {
    synapse_core::element_id(
        194,
        &format!(
            "0c0c01{:08x}{:08x}{:08x}{:08x}00000000",
            rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32
        ),
    )
}
fn bounds() -> Rect {
    Rect {
        x: 100,
        y: 200,
        w: 400,
        h: 300,
    }
}

#[test]
fn must_pass_ocr_center_inside_live_window_never_resolves_uia() {
    let id = ocr(Rect {
        x: 200,
        y: 300,
        w: 100,
        h: 40,
    });
    let calls = Cell::new(0);
    let point = element_center_with(
        &id,
        false,
        |root| {
            assert_eq!(root, 194);
            calls.set(calls.get() + 1);
            Ok(bounds())
        },
        |_, _| panic!("OCR must not resolve UIA"),
    )
    .expect("visible OCR point");
    assert_eq!(point, Point { x: 250, y: 320 });
    assert_eq!(calls.get(), 1);
}

#[test]
fn must_fail_ocr_outside_or_on_exclusive_window_edge_returns_no_click_point() {
    for rect in [
        Rect {
            x: 900,
            y: 300,
            w: 100,
            h: 40,
        },
        Rect {
            x: 490,
            y: 300,
            w: 20,
            h: 40,
        },
    ] {
        let id = ocr(rect);
        assert_eq!(crate::m1::browser_ocr_rect_from_element_id(&id), Some(rect));
        let error = element_center_with(
            &id,
            false,
            |_| Ok(bounds()),
            |_, _| panic!("OCR must not resolve UIA"),
        )
        .expect_err("outside window gate");
        let data = error.data.expect("structured refusal");
        assert_eq!(data["code"], "ACTION_TARGET_INVALID");
        assert_eq!(data["reason"], "target_click_point_outside_window");
        assert_eq!(data["tool"], "act_click");
    }
}

#[test]
fn must_fail_malformed_ocr_bbox_before_bounds_or_uia_resolution() {
    for rect in [
        Rect {
            x: 200,
            y: 300,
            w: 0,
            h: 40,
        },
        Rect {
            x: 200,
            y: 300,
            w: 100,
            h: -1,
        },
    ] {
        let id = ocr(rect);
        assert!(crate::m1::is_browser_ocr_element_id(&id));
        let error = element_center_with(
            &id,
            false,
            |_| panic!("invalid OCR must stop before bounds"),
            |_, _| panic!("invalid OCR must not fall through to UIA"),
        )
        .expect_err("malformed bbox refused");
        assert_eq!(error.data.expect("data")["code"], "ACTION_TARGET_INVALID");
    }
}

#[test]
fn must_pass_native_element_uses_refreshed_actionable_point() {
    let id = synapse_core::element_id(194, "01020304");
    let calls = Cell::new(0);
    let point = element_center_with(
        &id,
        false,
        |_| panic!("native actionability owns its bounds read"),
        |actual, root| {
            assert_eq!(actual, &id);
            assert_eq!(root, 194);
            calls.set(calls.get() + 1);
            Ok(Point { x: 250, y: 320 })
        },
    )
    .expect("refreshed native center");
    assert_eq!(point, Point { x: 250, y: 320 });
    assert_eq!(calls.get(), 1, "live gate cannot be bypassed");
}

#[test]
fn must_fail_native_gate_refusal_is_propagated_without_click_point() {
    let id = synapse_core::element_id(194, "01020304");
    let calls = Cell::new(0);
    let error = element_center_with(
        &id,
        false,
        |_| panic!("native route"),
        |_, _| {
            calls.set(calls.get() + 1);
            Err(crate::m1::mcp_error(
                "A11Y_ELEMENT_STALE",
                "synthetic live bbox refresh failed",
            ))
        },
    )
    .expect_err("stale native gate");
    assert_eq!(error.data.expect("data")["code"], "A11Y_ELEMENT_STALE");
    assert_eq!(calls.get(), 1);
}

#[test]
fn must_pass_recording_ocr_keeps_synthetic_hwnd_and_skips_live_window_bounds() {
    let id = ocr(Rect {
        x: 1500,
        y: 300,
        w: 100,
        h: 40,
    });
    let point = element_center_with(
        &id,
        true,
        |_| panic!("recording must permit synthetic HWNDs without live bounds"),
        |_, _| panic!("OCR recording must not resolve UIA"),
    )
    .expect("recording accepts synthetic coordinates");
    assert_eq!(point, Point { x: 1550, y: 320 });
}

#[test]
fn must_fail_recording_still_refuses_empty_ocr_bbox() {
    let id = ocr(Rect {
        x: 1500,
        y: 300,
        w: 0,
        h: 40,
    });
    let error = element_center_with(
        &id,
        true,
        |_| panic!("no live bounds for recording"),
        |_, _| panic!("no UIA for OCR"),
    )
    .expect_err("recording does not excuse an invalid bbox");
    assert_eq!(error.data.expect("data")["code"], "ACTION_TARGET_INVALID");
}
