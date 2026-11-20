//! #197: z-order aware child-window hit test for PostMessage delivery
//! (`act_click` element/point clicks and the `act_scroll at=` fallback).
//!
//! The old resolution picked the smallest-area visible descendant containing the
//! point. That ignores z-order, disabled state and `WS_EX_TRANSPARENT`, so with
//! overlapping sibling child windows (a docked panel over a canvas) the click or
//! wheel could reach a window the user cannot even see at that point.
//!
//! This walks the child tree top-down like `ChildWindowFromPointEx` with
//! `CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT`: at each level
//! the first sibling in z-order (topmost first) that contains the point and is
//! visible, enabled and not transparent wins, and the search continues inside
//! it. The selection rule itself is the pure [`topmost_hit_index`].

#[cfg(windows)]
use synapse_core::Point;
#[cfg(windows)]
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::{
        GW_CHILD, GW_HWNDNEXT, GWL_EXSTYLE, GWL_STYLE, GetWindow, GetWindowLongPtrW, GetWindowRect,
        IsWindowVisible, WINDOW_LONG_PTR_INDEX, WS_DISABLED, WS_EX_TRANSPARENT,
    },
};

/// Facts about one sibling child window that decide whether it can be hit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HitCandidate {
    /// The window rect has area and contains the screen point.
    pub(crate) contains_point: bool,
    pub(crate) visible: bool,
    pub(crate) enabled: bool,
    /// `WS_EX_TRANSPARENT`: mouse hit-testing passes through to windows behind it.
    pub(crate) transparent: bool,
}

impl HitCandidate {
    pub(crate) const fn can_receive_input(self) -> bool {
        self.contains_point && self.visible && self.enabled && !self.transparent
    }
}

/// Index of the sibling that receives input at the point. `siblings_top_to_bottom`
/// is in z-order, topmost first; the first hittable sibling wins, regardless of
/// area. `None` means no child is hit and the parent itself is the target.
pub(crate) fn topmost_hit_index(siblings_top_to_bottom: &[HitCandidate]) -> Option<usize> {
    siblings_top_to_bottom
        .iter()
        .position(|candidate| candidate.can_receive_input())
}

#[cfg(windows)]
const MAX_HIT_TEST_DEPTH: usize = 64;
#[cfg(windows)]
const MAX_SIBLINGS_PER_LEVEL: usize = 4096;

/// Deepest hittable descendant of `root` at the screen `point`, or `root` itself
/// when no child window is hit.
#[cfg(windows)]
pub(crate) fn deepest_hit_hwnd(root: HWND, point: Point) -> HWND {
    let mut current = root;
    for _ in 0..MAX_HIT_TEST_DEPTH {
        let siblings = children_top_to_bottom(current);
        let candidates: Vec<HitCandidate> = siblings
            .iter()
            .map(|hwnd| hit_candidate(*hwnd, point))
            .collect();
        let Some(index) = topmost_hit_index(&candidates) else {
            break;
        };
        current = siblings[index];
    }
    current
}

#[cfg(windows)]
fn children_top_to_bottom(parent: HWND) -> Vec<HWND> {
    let mut children = Vec::new();
    // SAFETY: HWND is a plain handle value that is never dereferenced; GetWindow only reads it
    // and returns Err for a stale handle or when there is no such child.
    let mut next = unsafe { GetWindow(parent, GW_CHILD) }.ok();
    while let Some(child) = next {
        if children.len() >= MAX_SIBLINGS_PER_LEVEL {
            break;
        }
        children.push(child);
        // SAFETY: as above; GW_HWNDNEXT walks to the next sibling lower in z-order.
        next = unsafe { GetWindow(child, GW_HWNDNEXT) }.ok();
    }
    children
}

#[cfg(windows)]
fn hit_candidate(hwnd: HWND, point: Point) -> HitCandidate {
    let mut rect = RECT::default();
    // SAFETY: the out-pointer refers to the live local `rect`; a stale HWND makes the call
    // return an error, which is treated as "does not contain the point".
    let contains_point = unsafe { GetWindowRect(hwnd, &raw mut rect) }.is_ok()
        && point.x >= rect.left
        && point.x < rect.right
        && point.y >= rect.top
        && point.y < rect.bottom
        && rect.right > rect.left
        && rect.bottom > rect.top;
    // SAFETY: HWND is a plain handle value; IsWindowVisible only reads it and returns FALSE for an
    // invalid handle.
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    HitCandidate {
        contains_point,
        visible,
        enabled: style_bits(hwnd, GWL_STYLE) & WS_DISABLED.0 == 0,
        transparent: style_bits(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 != 0,
    }
}

/// Low 32 bits of a window style slot; styles are 32-bit flag words.
#[cfg(windows)]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn style_bits(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX) -> u32 {
    // SAFETY: HWND is a plain handle value that is never dereferenced; a stale handle returns 0.
    let value = unsafe { GetWindowLongPtrW(hwnd, index) };
    value as u32
}
