//! FROZEN behaviour table of Gitea #2350 / #2359 (`act verb=drag`): rows
//! D1-D6 (`DragRoute`) and P1-P7 (`DragPath`), as the issue table, comment
//! 110203. Written from the issue table before the code. Coders never edit
//! this file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/server/drag_route_table_2350.rs (registered
//! by the `#[cfg(test)] #[path]` lines at the end of drag_route.rs).
//! One test per row, named after the row. CONTROL cases: a row that expects
//! a refusal would pass on a stub that refuses everything with the row's
//! code (D4, D5, D6, P3, P4, P5, P6, P7's refusals), and a row that expects
//! one backend would pass on a stub that always answers it (D1, D2, D3);
//! each such row carries the opposite neighbour, so no single-answer stub
//! passes it.
//! Coordinates: a window target's area is its outer rect on the screen and a
//! caller point is counted from its top-left corner; a raw-CDP page's area is
//! its viewport from 0,0 (CSS pixels). An element's box is in the area's
//! pixels; a raw-CDP element is read only by its centre (a box of size 0).

use rmcp::ErrorData;
use serde_json::Value;
use synapse_core::{Point, Rect, error_codes};

use super::super::act_route::ActTarget;
use super::super::element_scope::{ElementScope, ScopeElement, ScopeTarget};
use super::{DragArea, DragBackend, DragEnd, DragPath, DragRoute};
use crate::server::SessionTarget;

/// Window W1 (the target), window W2 (another window), a child HWND inside
/// each (a native element's own HWND differs from its root).
const W1: i64 = 0x1_0010;
const W2: i64 = 0x2_0020;
const W1_CHILD: i64 = 0x1_0011;
const W2_CHILD: i64 = 0x2_0021;
/// DevTools page ids: P1 is the session's page, P2 another page.
const P1: &str = "A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1";
const P2: &str = "B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2";
/// W1's outer rect on the screen.
const WINDOW: Rect = Rect {
    x: 100,
    y: 50,
    w: 800,
    h: 600,
};
/// P1's viewport.
const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 1280,
    h: 720,
};
const NATIVE_KINDS: [ActTarget; 2] = [ActTarget::NativeWindow, ActTarget::NativeWindowWithRawCdp];
const FLAGS: [(bool, bool); 4] = [(true, true), (true, false), (false, true), (false, false)];

fn code(error: &ErrorData) -> &str {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn refused<T: std::fmt::Debug>(row: &str, result: Result<T, ErrorData>) -> ErrorData {
    match result {
        Ok(value) => panic!("{row}: expected a refusal, got {value:?}"),
        Err(error) => error,
    }
}

fn accepted<T>(row: &str, result: Result<T, ErrorData>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!(
            "{row}: expected no refusal, got {} {}",
            code(&error),
            error.message
        ),
    }
}

fn backend(target: ActTarget, claimed: bool, in_front: bool) -> Option<DragBackend> {
    DragRoute::new(target, claimed, in_front).backend().ok()
}

fn point(x: i32, y: i32) -> Point {
    Point { x, y }
}

fn at(x: i32, y: i32) -> Option<DragEnd> {
    Some(DragEnd::Point(point(x, y)))
}

fn selector(value: &str) -> Option<DragEnd> {
    Some(DragEnd::Selector(value.to_owned()))
}

/// A UIA element of window `root` (its own HWND `element_hwnd`), judged
/// against the session target W1, with its screen box.
fn native(element_hwnd: i64, root: i64, bounds: Rect) -> Option<DragEnd> {
    let scope = ElementScope::new(
        ScopeTarget::new(Some(SessionTarget::Window { hwnd: W1 }), Vec::new()),
        ScopeElement::new(
            &format!("0x{element_hwnd:x}:2a00000001"),
            Some(root),
            Vec::new(),
        ),
    );
    Some(DragEnd::Element { scope, bounds })
}

/// A web element of page `page` in W1, judged against the session target
/// page P1 of W1, read by its viewport centre.
fn web(page: &str, centre_x: i32, centre_y: i32) -> Option<DragEnd> {
    let id = synapse_a11y::cdp_element_id_for_target(W1, page, 42);
    let scope = ElementScope::new(
        ScopeTarget::new(
            Some(SessionTarget::Cdp {
                window_hwnd: W1,
                cdp_target_id: P1.to_owned(),
            }),
            Vec::new(),
        ),
        ScopeElement::new(id.as_str(), None, Vec::new()),
    );
    Some(DragEnd::Element {
        scope,
        bounds: Rect {
            x: centre_x,
            y: centre_y,
            w: 0,
            h: 0,
        },
    })
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn d1_bridge_tab_gets_the_bridge_drag() {
    for (claimed, in_front) in FLAGS {
        assert_eq!(
            backend(ActTarget::BridgeTab, claimed, in_front),
            Some(DragBackend::BridgeDrag),
            "D1 claimed={claimed} in_front={in_front}"
        );
    }
    // CONTROL: a raw-CDP page is not served by the bridge.
    assert_ne!(
        backend(ActTarget::RawCdpPage, true, true),
        Some(DragBackend::BridgeDrag),
        "D1 control"
    );
}

#[test]
fn d2_raw_cdp_page_gets_the_raw_cdp_stroke() {
    for (claimed, in_front) in FLAGS {
        assert_eq!(
            backend(ActTarget::RawCdpPage, claimed, in_front),
            Some(DragBackend::RawCdpStroke),
            "D2 claimed={claimed} in_front={in_front}"
        );
    }
    // CONTROL: a bridge tab is not stroked over raw CDP.
    assert_ne!(
        backend(ActTarget::BridgeTab, true, true),
        Some(DragBackend::RawCdpStroke),
        "D2 control"
    );
}

#[test]
fn d3_claimed_native_window_in_front_gets_the_real_input_stroke() {
    for kind in NATIVE_KINDS {
        assert_eq!(
            backend(kind, true, true),
            Some(DragBackend::RealInputStroke),
            "D3 {kind:?}"
        );
    }
    // CONTROL: a bridge tab never gets real input.
    assert_ne!(
        backend(ActTarget::BridgeTab, true, true),
        Some(DragBackend::RealInputStroke),
        "D3 control"
    );
}

#[test]
fn d4_claimed_native_window_not_in_front_is_refused() {
    for kind in NATIVE_KINDS {
        let error = refused("D4", DragRoute::new(kind, true, false).backend());
        assert_eq!(
            code(&error),
            error_codes::TOOL_PROFILE_POLICY_DENIED,
            "D4 {kind:?}"
        );
        assert!(
            error.message.contains("in front"),
            "D4 {kind:?}: the text says the window must be in front: {}",
            error.message
        );
        assert!(
            error.message.contains("nothing was sent"),
            "D4 {kind:?}: {}",
            error.message
        );
    }
    // CONTROL: the same window in front is served.
    assert_eq!(
        backend(ActTarget::NativeWindow, true, true),
        Some(DragBackend::RealInputStroke),
        "D4 control"
    );
}

#[test]
fn d5_native_window_not_claimed_by_this_session_is_refused() {
    for kind in NATIVE_KINDS {
        for in_front in [true, false] {
            let error = refused("D5", DragRoute::new(kind, false, in_front).backend());
            assert_eq!(
                code(&error),
                error_codes::TARGET_CLAIM_NOT_FOUND,
                "D5 {kind:?} in_front={in_front}"
            );
        }
    }
    // CONTROL: the claimed window in front is served.
    assert_eq!(
        backend(ActTarget::NativeWindow, true, true),
        Some(DragBackend::RealInputStroke),
        "D5 control"
    );
}

#[test]
fn d6_no_target_is_refused_target_not_set() {
    for (claimed, in_front) in FLAGS {
        let error = refused(
            "D6",
            DragRoute::new(ActTarget::None, claimed, in_front).backend(),
        );
        assert_eq!(
            code(&error),
            error_codes::TARGET_NOT_SET,
            "D6 claimed={claimed} in_front={in_front}"
        );
    }
    // CONTROL: a bridge tab target is served.
    assert_eq!(
        backend(ActTarget::BridgeTab, false, false),
        Some(DragBackend::BridgeDrag),
        "D6 control"
    );
}

#[test]
fn p1_element_to_element_goes_centre_to_centre() {
    let path = accepted(
        "P1 window",
        DragPath::new(
            native(W1_CHILD, W1, rect(200, 100, 40, 20)),
            native(W1_CHILD, W1, rect(500, 300, 60, 30)),
            None,
            DragArea::Bounds(WINDOW),
        ),
    );
    assert_eq!(
        path.points(),
        Some((point(220, 110), point(530, 315))),
        "P1 window"
    );
    let path = accepted(
        "P1 page",
        DragPath::new(
            web(P1, 300, 200),
            web(P1, 600, 400),
            None,
            DragArea::Bounds(VIEWPORT),
        ),
    );
    assert_eq!(
        path.points(),
        Some((point(300, 200), point(600, 400))),
        "P1 page"
    );
}

#[test]
fn p2_point_to_point_is_that_path() {
    // A window point counts from the window's top-left corner.
    let path = accepted(
        "P2 window",
        DragPath::new(at(10, 20), at(400, 300), None, DragArea::Bounds(WINDOW)),
    );
    assert_eq!(
        path.points(),
        Some((point(110, 70), point(500, 350))),
        "P2 window"
    );
    assert_eq!(path.selectors(), None, "P2 window");
    let path = accepted(
        "P2 page",
        DragPath::new(at(10, 20), at(400, 300), None, DragArea::Bounds(VIEWPORT)),
    );
    assert_eq!(
        path.points(),
        Some((point(10, 20), point(400, 300))),
        "P2 page"
    );
    // A Chrome bridge tab: the bridge drag takes two selectors, as given.
    let path = accepted(
        "P2 bridge",
        DragPath::new(
            selector("#card"),
            selector("#zone"),
            None,
            DragArea::BridgeTab,
        ),
    );
    assert_eq!(path.selectors(), Some(("#card", "#zone")), "P2 bridge");
    assert_eq!(path.points(), None, "P2 bridge");
    // An end of the wrong kind for the area is refused.
    for (case, from, to, area) in [
        (
            "bridge point",
            at(10, 20),
            selector("#zone"),
            DragArea::BridgeTab,
        ),
        (
            "window selector",
            selector("#card"),
            at(400, 300),
            DragArea::Bounds(WINDOW),
        ),
    ] {
        let error = refused(case, DragPath::new(from, to, None, area));
        assert_eq!(code(&error), error_codes::TOOL_PARAMS_INVALID, "P2 {case}");
    }
}

#[test]
fn p3_both_ends_are_needed() {
    for (case, from, to, area) in [
        ("start only", at(10, 20), None, DragArea::Bounds(WINDOW)),
        ("end only", None, at(400, 300), DragArea::Bounds(WINDOW)),
        ("none", None, None, DragArea::Bounds(WINDOW)),
        (
            "bridge start only",
            selector("#card"),
            None,
            DragArea::BridgeTab,
        ),
    ] {
        let error = refused(case, DragPath::new(from, to, None, area));
        assert_eq!(code(&error), error_codes::TOOL_PARAMS_INVALID, "P3 {case}");
        assert!(
            error.message.contains("both ends"),
            "P3 {case}: {}",
            error.message
        );
    }
    // CONTROL: both ends given.
    accepted(
        "P3 control",
        DragPath::new(at(10, 20), at(400, 300), None, DragArea::Bounds(WINDOW)),
    );
}

#[test]
fn p4_start_equal_to_end_is_refused() {
    for (case, from, to, area) in [
        (
            "same point",
            at(10, 20),
            at(10, 20),
            DragArea::Bounds(WINDOW),
        ),
        (
            "same element",
            native(W1_CHILD, W1, rect(200, 100, 40, 20)),
            native(W1_CHILD, W1, rect(200, 100, 40, 20)),
            DragArea::Bounds(WINDOW),
        ),
        (
            "same selector",
            selector("#card"),
            selector("#card"),
            DragArea::BridgeTab,
        ),
    ] {
        let error = refused(case, DragPath::new(from, to, None, area));
        assert_eq!(code(&error), error_codes::TOOL_PARAMS_INVALID, "P4 {case}");
        assert!(
            error.message.contains("nothing to drag"),
            "P4 {case}: {}",
            error.message
        );
    }
    // CONTROL: one pixel apart is a drag.
    accepted(
        "P4 control",
        DragPath::new(at(10, 20), at(11, 20), None, DragArea::Bounds(WINDOW)),
    );
}

#[test]
fn p5_end_point_outside_the_target_is_refused() {
    for (case, from, to, area) in [
        // x = 800 is the window's right edge (exclusive).
        (
            "right edge",
            at(10, 20),
            at(800, 10),
            DragArea::Bounds(WINDOW),
        ),
        (
            "left of window",
            at(-1, 20),
            at(400, 300),
            DragArea::Bounds(WINDOW),
        ),
        (
            "below window",
            at(10, 20),
            at(10, 600),
            DragArea::Bounds(WINDOW),
        ),
        (
            "right of viewport",
            at(10, 20),
            at(1280, 0),
            DragArea::Bounds(VIEWPORT),
        ),
        (
            "element centre outside",
            at(10, 20),
            native(W1_CHILD, W1, rect(880, 600, 40, 20)),
            DragArea::Bounds(WINDOW),
        ),
    ] {
        let error = refused(case, DragPath::new(from, to, None, area));
        assert_eq!(
            code(&error),
            error_codes::ACTION_TARGET_INVALID,
            "P5 {case}"
        );
    }
    // CONTROL: the last pixel inside is allowed.
    let path = accepted(
        "P5 control",
        DragPath::new(at(0, 0), at(799, 599), None, DragArea::Bounds(WINDOW)),
    );
    assert_eq!(
        path.points(),
        Some((point(100, 50), point(899, 649))),
        "P5 control"
    );
}

#[test]
fn p6_end_element_of_another_window_or_page_is_refused() {
    let error = refused(
        "P6 window",
        DragPath::new(
            at(10, 20),
            native(W2_CHILD, W2, rect(500, 300, 60, 30)),
            None,
            DragArea::Bounds(WINDOW),
        ),
    );
    assert_eq!(
        code(&error),
        error_codes::ACTION_TARGET_INVALID,
        "P6 window"
    );
    assert!(
        error.message.contains("another window"),
        "P6 window: {}",
        error.message
    );
    let error = refused(
        "P6 page",
        DragPath::new(
            web(P1, 300, 200),
            web(P2, 600, 400),
            None,
            DragArea::Bounds(VIEWPORT),
        ),
    );
    assert_eq!(code(&error), error_codes::ACTION_TARGET_INVALID, "P6 page");
    assert!(
        error.message.contains("another page"),
        "P6 page: {}",
        error.message
    );
    // CONTROL: the same box inside the target window is allowed.
    accepted(
        "P6 control",
        DragPath::new(
            at(10, 20),
            native(W1_CHILD, W1, rect(500, 300, 60, 30)),
            None,
            DragArea::Bounds(WINDOW),
        ),
    );
}

#[test]
fn p7_duration_bounds_and_default() {
    for requested in [0, 10_001, -1] {
        let error = refused(
            "P7",
            DragPath::new(
                at(10, 20),
                at(400, 300),
                Some(requested),
                DragArea::Bounds(WINDOW),
            ),
        );
        assert_eq!(
            code(&error),
            error_codes::TOOL_PARAMS_INVALID,
            "P7 duration {requested}"
        );
    }
    for (requested, expected) in [(None, 350), (Some(10_000), 10_000), (Some(1), 1)] {
        let path = accepted(
            "P7",
            DragPath::new(
                at(10, 20),
                at(400, 300),
                requested,
                DragArea::Bounds(WINDOW),
            ),
        );
        assert_eq!(path.duration_ms(), expected, "P7 duration {requested:?}");
    }
}
