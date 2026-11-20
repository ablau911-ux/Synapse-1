//! #233: a hidden-desktop `read_text` reports and reads the clipped client
//! region the desktop worker returns, not the unclipped request.

use synapse_core::Rect;

use super::*;

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn must_pass_worker_clipped_client_region_replaces_the_requested_region() {
    let plan = hidden_desktop_read_text_capture_plan(Some(rect(0, 0, 600, 120)));
    assert_eq!(
        hidden_desktop_read_text_request_region(&plan, Some(rect(1, 1, 599, 119))),
        rect(1, 1, 599, 119)
    );
}

/// When the worker reports no clipped region for a client-region plan, the
/// plan's own request region stands (nothing to replace it with).
#[test]
fn must_pass_region_plan_without_worker_region_keeps_request() {
    let requested = rect(10, 20, 300, 40);
    let plan = hidden_desktop_read_text_capture_plan(Some(requested));
    assert_eq!(plan.capture_region, Some(requested));
    assert!(plan.client_region);
    assert!(!plan.whole_window);
    assert_eq!(
        hidden_desktop_read_text_request_region(&plan, None),
        requested
    );
}

#[test]
fn must_pass_whole_window_plan_keeps_its_placeholder() {
    let plan = hidden_desktop_read_text_capture_plan(None);
    assert_eq!(
        hidden_desktop_read_text_request_region(&plan, None),
        rect(0, 0, 0, 0)
    );
}
