//! Chooses which window a physical input event is attributed to (#106).
//!
//! Keyboard input belongs to the foreground window. Mouse input (click, wheel)
//! belongs to the top-level window under the pointer, which is not necessarily
//! the foreground window (wheel scrolling an unfocused window is common).

use synapse_core::{ForegroundContext, Point};

use super::interaction_cadence::{InteractionEvent, InteractionEventKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WindowSource {
    Foreground,
    TopLevelAtPoint { x: i32, y: i32 },
}

pub(super) fn window_source(event: &InteractionEvent) -> WindowSource {
    match (event.kind, event.screen_point) {
        (
            InteractionEventKind::Click
            | InteractionEventKind::VerticalScroll { .. }
            | InteractionEventKind::HorizontalScroll { .. },
            Some((x, y)),
        ) => WindowSource::TopLevelAtPoint { x, y },
        _ => WindowSource::Foreground,
    }
}

/// Resolves the window a physical (non-injected) event belongs to. Falls back
/// to the foreground window when the pointer is not over a readable window.
pub(super) fn resolve_user_window(event: &InteractionEvent) -> Option<ForegroundContext> {
    if let WindowSource::TopLevelAtPoint { x, y } = window_source(event)
        && let Some(hwnd) = crate::m2::root_hwnd_at_point(Point { x, y })
        && let Ok(context) = synapse_a11y::foreground_context(hwnd)
    {
        return Some(context);
    }
    synapse_a11y::current_foreground_context().ok()
}

#[cfg(test)]
#[path = "tests/input_window_106.rs"]
mod input_window_106_tests;
