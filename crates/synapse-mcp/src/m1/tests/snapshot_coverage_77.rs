//! #77: a partial accessibility snapshot must never be reported as healthy,
//! on any observe path, and `find` must carry the same coverage verdict.

use synapse_core::{AccessibleSubtree, PerceptionMode, Rect, SensorStatus};

use super::*;
use crate::m1::{FindParams, match_find_input};

/// A window root with one real content child, so only `truncated` decides the coverage verdict.
fn tree(truncated: bool) -> AccessibleSubtree {
    let mut root = node(0, 0, "Window", "Window", false);
    root.children_count = 1;
    AccessibleSubtree {
        root: root.element_id.clone(),
        nodes: vec![root, node(1, 1, "Content", "Edit", false)],
        max_depth: 2,
        truncated,
        selected_content_root: None,
        completeness_scope: None,
        stop_reason: None,
        skipped_subtree_roots: Vec::new(),
    }
}

fn foreground() -> ForegroundContext {
    foreground_with_bounds(Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    })
}

fn truncated_status() -> SensorStatus {
    SensorStatus::DegradedSensorFailed {
        reason_code: A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED.to_owned(),
    }
}

#[cfg(windows)]
#[test]
fn must_fail_hidden_desktop_observe_reports_truncated_snapshot_as_degraded() {
    let input = hidden_desktop_input_from_worker_snapshot(
        tree(true),
        foreground(),
        PerceptionMode::A11yOnly,
    );
    assert_eq!(input.a11y_status, truncated_status());
}

#[cfg(windows)]
#[test]
fn must_pass_hidden_desktop_observe_reports_complete_snapshot_as_healthy() {
    let input = hidden_desktop_input_from_worker_snapshot(
        tree(false),
        foreground(),
        PerceptionMode::A11yOnly,
    );
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

#[test]
fn must_fail_find_carries_the_coverage_reason_of_a_degraded_snapshot() {
    let mut input = ObservationInput::new(foreground());
    input.a11y_status = truncated_status();
    let response = match_find_input(&input, &FindParams::default());
    assert_eq!(
        response.a11y_coverage.as_deref(),
        Some(A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED)
    );
}

#[test]
fn must_pass_find_omits_coverage_for_a_healthy_snapshot() {
    let mut input = ObservationInput::new(foreground());
    input.a11y_status = SensorStatus::Healthy;
    let response = match_find_input(&input, &FindParams::default());
    assert_eq!(response.a11y_coverage, None);
}
