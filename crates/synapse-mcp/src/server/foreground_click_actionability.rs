//! #194: shared actionability gate for Chromium foreground tiers that click the
//! centre of a UIA bbox with the real cursor (`act_type` `into_element` fallback and
//! the `act_set_field_text` foreground tier).
//!
//! Playwright-style discipline: scroll the element into view, re-read its live
//! bbox, and require the click point to land inside the target window. A stale or
//! below-the-fold bbox must never steer a real-cursor click into another window.

use synapse_core::{ElementId, Point, Rect, UiaPattern, error_codes};

use super::rect_contains_point;

/// Why a bbox centre may not be clicked. Callers map this onto their own structured
/// error (`code` + `reason` + human `detail`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClickPointRefusal {
    pub code: &'static str,
    pub reason: &'static str,
    pub detail: String,
}

/// Centre of `bbox`, which must lie inside `window_bounds`.
pub(crate) fn visible_click_point(
    bbox: Rect,
    window_bounds: Rect,
) -> Result<Point, ClickPointRefusal> {
    if bbox.w <= 0 || bbox.h <= 0 {
        return Err(ClickPointRefusal {
            code: error_codes::ACTION_TARGET_INVALID,
            reason: "target_bbox_empty",
            detail: "target bbox is empty or inverted".to_owned(),
        });
    }
    let x = i64::from(bbox.x) + i64::from(bbox.w) / 2;
    let y = i64::from(bbox.y) + i64::from(bbox.h) / 2;
    let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
        return Err(ClickPointRefusal {
            code: error_codes::ACTION_TARGET_INVALID,
            reason: "target_bbox_center_overflow",
            detail: format!("target bbox centre ({x}, {y}) overflowed i32"),
        });
    };
    let point = Point { x, y };
    if !rect_contains_point(window_bounds, point) {
        return Err(ClickPointRefusal {
            code: error_codes::ACTION_TARGET_INVALID,
            reason: "target_click_point_outside_window",
            detail: format!(
                "click point ({}, {}) is outside the target window bounds ({}, {}, {}x{}); the element is not visible in the viewport and could not be scrolled into it",
                point.x,
                point.y,
                window_bounds.x,
                window_bounds.y,
                window_bounds.w,
                window_bounds.h
            ),
        });
    }
    Ok(point)
}

/// Live UIA gate: scroll the element into view when it supports `ScrollItem`,
/// refresh `bbox` from UIA, and return its centre only if it lies inside the
/// target window `root_hwnd`. `bbox` is updated even when the call is refused.
pub(crate) fn actionable_click_point(
    element_id: &ElementId,
    root_hwnd: i64,
    patterns: &[UiaPattern],
    bbox: &mut Rect,
) -> Result<Point, ClickPointRefusal> {
    let scrolled = patterns.contains(&UiaPattern::ScrollItem);
    if scrolled {
        synapse_a11y::scroll_element_into_view(element_id).map_err(|error| ClickPointRefusal {
            code: error.code(),
            reason: "scroll_into_view_failed",
            detail: format!(
                "ScrollItemPattern scroll-into-view failed for element {element_id}: {error}"
            ),
        })?;
    }
    *bbox = synapse_a11y::element_bounding_rect(element_id).map_err(|error| ClickPointRefusal {
        code: error.code(),
        reason: "bbox_refresh_failed",
        detail: format!("live bounding-rect readback failed for element {element_id}: {error}"),
    })?;
    tracing::info!(
        code = "M2_FOREGROUND_CLICK_BBOX_REFRESHED",
        element_id = %element_id,
        scrolled_into_view = scrolled,
        bbox_x = bbox.x,
        bbox_y = bbox.y,
        bbox_w = bbox.w,
        bbox_h = bbox.h,
        "readback=bbox_refresh tool=foreground_click_actionability"
    );
    let window_bounds = synapse_a11y::foreground_context(root_hwnd)
        .map_err(|error| ClickPointRefusal {
            code: error.code(),
            reason: "window_bounds_read_failed",
            detail: format!(
                "target window bounds readback failed for hwnd 0x{root_hwnd:x}: {error}"
            ),
        })?
        .window_bounds;
    visible_click_point(*bbox, window_bounds)
}
