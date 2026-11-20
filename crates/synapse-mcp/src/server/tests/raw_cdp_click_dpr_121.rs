//! #121: a window/screenshot-pixel coordinate click on a WebView2 page is
//! converted to CSS viewport pixels (divide by devicePixelRatio after removing
//! the host client origin) and range-checked BEFORE `Input.dispatchMouseEvent`.
//! Values from the issue's live FSV (DPR 1.25, CSS viewport 1200x800: window
//! x=1351,y=100 -> CSS 1073.6,49.6; DPR 1.0 viewport 1500x1000: CSS 1364,50;
//! right edge x=1200 and x=-1 refused).
//!
//! NEEDS-SEAM (fixer extracts the pure conversion from
//! `target_act_raw_cdp_coordinate_click`, behaviour unchanged):
//!   fn raw_cdp_click_css_point(
//!       space: TargetActCoordinateSpace,
//!       x: i32,
//!       y: i32,
//!       client_physical: Option<synapse_core::Point>, // screen point minus client origin
//!       width: f64,
//!       height: f64,
//!       dpr: f64,
//!   ) -> Result<synapse_a11y::CdpActionPoint, ErrorData>
//! Viewport space uses x/y as CSS px; window/screen space requires
//! `client_physical` (None -> ACTION_TARGET_INVALID).

use synapse_core::Point;

use super::*;

fn css(
    space: TargetActCoordinateSpace,
    xy: (i32, i32),
    client: Option<(i32, i32)>,
    viewport: (f64, f64, f64),
) -> Result<synapse_a11y::CdpActionPoint, ErrorData> {
    raw_cdp_click_css_point(
        space,
        xy.0,
        xy.1,
        client.map(|(x, y)| Point { x, y }),
        viewport.0,
        viewport.1,
        viewport.2,
    )
}

fn assert_point(point: synapse_a11y::CdpActionPoint, x: f64, y: f64) {
    assert!(
        (point.x - x).abs() < 1e-9 && (point.y - y).abs() < 1e-9,
        "expected ({x}, {y}), got ({}, {})",
        point.x,
        point.y
    );
}

fn refused(result: Result<synapse_a11y::CdpActionPoint, ErrorData>) {
    let error = result.expect_err("must be refused before dispatch");
    assert_eq!(
        error.data.as_ref().and_then(|data| data["code"].as_str()),
        Some(error_codes::ACTION_TARGET_INVALID),
        "{error:?}"
    );
}

const DPR_125: (f64, f64, f64) = (1200.0, 800.0, 1.25);
const DPR_100: (f64, f64, f64) = (1500.0, 1000.0, 1.0);

#[test]
fn must_pass_window_pixels_at_dpr_125_are_divided_into_css_pixels() {
    // Live FSV: window x=1351,y=100 -> client (1342, 62) -> CSS (1073.6, 49.6).
    let point = css(
        TargetActCoordinateSpace::Window,
        (1351, 100),
        Some((1342, 62)),
        DPR_125,
    )
    .expect("inside the viewport");
    assert_point(point, 1073.6, 49.6);
}

#[test]
fn must_pass_window_pixels_at_dpr_100_are_unchanged() {
    let point = css(
        TargetActCoordinateSpace::Window,
        (1373, 88),
        Some((1364, 50)),
        DPR_100,
    )
    .expect("inside the viewport");
    assert_point(point, 1364.0, 50.0);
}

#[test]
fn must_pass_viewport_space_is_already_css_and_ignores_dpr() {
    let point = css(
        TargetActCoordinateSpace::Viewport,
        (1073, 50),
        None,
        DPR_125,
    )
    .expect("inside the viewport");
    assert_point(point, 1073.0, 50.0);
    let last = css(
        TargetActCoordinateSpace::Viewport,
        (1199, 799),
        None,
        DPR_125,
    )
    .expect("last CSS pixel is inside");
    assert_point(last, 1199.0, 799.0);
}

#[test]
fn must_fail_right_and_bottom_edges_and_negatives_are_refused() {
    refused(css(
        TargetActCoordinateSpace::Viewport,
        (1200, 50),
        None,
        DPR_125,
    ));
    refused(css(
        TargetActCoordinateSpace::Viewport,
        (50, 800),
        None,
        DPR_125,
    ));
    refused(css(
        TargetActCoordinateSpace::Viewport,
        (-1, 50),
        None,
        DPR_125,
    ));
    // 1500 physical / 1.25 = 1200 CSS: exactly the right edge.
    refused(css(
        TargetActCoordinateSpace::Window,
        (1509, 100),
        Some((1500, 62)),
        DPR_125,
    ));
}

#[test]
fn must_fail_invalid_viewport_metrics_are_refused() {
    for viewport in [
        (1200.0, 800.0, 0.0),
        (1200.0, 800.0, f64::NAN),
        (0.0, 800.0, 1.25),
        (1200.0, f64::INFINITY, 1.25),
    ] {
        refused(css(
            TargetActCoordinateSpace::Window,
            (100, 100),
            Some((100, 100)),
            viewport,
        ));
    }
}

#[test]
fn must_fail_window_space_without_a_client_origin_is_refused() {
    refused(css(
        TargetActCoordinateSpace::Window,
        (100, 100),
        None,
        DPR_125,
    ));
}

/// Wiring: the raw-CDP click converts and validates before capture/dispatch.
#[test]
fn must_pass_raw_cdp_click_converts_before_any_cdp_input() {
    let source = include_str!("../background_router.rs");
    let start = source
        .find("async fn target_act_raw_cdp_coordinate_click(")
        .expect("raw CDP click present");
    let body = &source[start..];
    let body = &body[..body.find("\n}\n").expect("function end")];
    let convert = body
        .find("raw_cdp_click_css_point(")
        .expect("conversion goes through the seam");
    for later in ["cdp_capture_page_bgra(", "cdp_click_point_target("] {
        let at = body
            .find(later)
            .unwrap_or_else(|| panic!("{later} present"));
        assert!(convert < at, "conversion must precede {later}");
    }
}
