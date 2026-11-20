//! Element-to-window crop math for `read_text(element_id=...)` (#178).
//!
//! An element's UIA rectangle is in screen coordinates. Capturing those screen pixels reads
//! whatever window covers the element, so the element's own window is captured instead
//! (per-window WGC) and the element rectangle is cropped out of that frame. The window frame
//! (`window_capture_bounds`, the DWM extended frame) is the origin of the WGC window bitmap.

use synapse_core::Rect;

/// Where an element sits inside its own window's capture bitmap.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ElementWindowCrop {
    /// The part of the element that lies inside the window frame, in screen coordinates.
    pub screen_region: Rect,
    /// The same part relative to the window frame's top-left (window bitmap coordinates).
    pub window_region: Rect,
}

/// Intersects `element` (screen coordinates) with `frame` (the window's screen-space capture
/// bounds) and maps the result into window bitmap coordinates.
///
/// Returns `None` when either rectangle is empty or they do not overlap, meaning no pixel of
/// the element belongs to the window and nothing may be read for it.
#[must_use]
pub fn crop_element_to_window_frame(element: Rect, frame: Rect) -> Option<ElementWindowCrop> {
    if element.w <= 0 || element.h <= 0 || frame.w <= 0 || frame.h <= 0 {
        return None;
    }
    let left = i64::from(element.x).max(i64::from(frame.x));
    let top = i64::from(element.y).max(i64::from(frame.y));
    let right =
        (i64::from(element.x) + i64::from(element.w)).min(i64::from(frame.x) + i64::from(frame.w));
    let bottom =
        (i64::from(element.y) + i64::from(element.h)).min(i64::from(frame.y) + i64::from(frame.h));
    if right <= left || bottom <= top {
        return None;
    }
    let width = i32::try_from(right - left).ok()?;
    let height = i32::try_from(bottom - top).ok()?;
    Some(ElementWindowCrop {
        screen_region: Rect {
            x: i32::try_from(left).ok()?,
            y: i32::try_from(top).ok()?,
            w: width,
            h: height,
        },
        window_region: Rect {
            x: i32::try_from(left - i64::from(frame.x)).ok()?,
            y: i32::try_from(top - i64::from(frame.y)).ok()?,
            w: width,
            h: height,
        },
    })
}

#[cfg(test)]
#[path = "tests/element_window_crop_178.rs"]
mod tests;
