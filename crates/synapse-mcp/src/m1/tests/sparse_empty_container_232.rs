//! #232: a target window whose only non-chrome nodes are empty, childless
//! structural containers (a CEF/Chromium render-host `pane`) exposes no usable
//! content and must not be reported as a healthy accessibility snapshot.

use synapse_core::{AccessibleNode, Rect, SensorStatus, UiaPattern};

use super::*;

fn input_with(children: Vec<AccessibleNode>) -> ObservationInput {
    let mut input = ObservationInput::new(foreground_with_bounds(Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    }));
    let mut root = node(0, 0, "Spotify Premium", "window", false);
    root.children_count = u32::try_from(children.len()).unwrap_or(u32::MAX);
    input.elements = std::iter::once(root).chain(children).collect();
    input.a11y_status = SensorStatus::Healthy;
    input
}

fn no_content() -> SensorStatus {
    SensorStatus::DegradedSensorFailed {
        reason_code: A11Y_TARGET_WINDOW_NO_UIA_CONTENT.to_owned(),
    }
}

#[test]
fn must_fail_window_whose_only_content_is_an_empty_childless_pane_is_not_healthy() {
    let mut input = input_with(vec![
        node(1, 1, "", "pane", false),
        node(2, 1, "", "title bar", false),
    ]);
    mark_sparse_target_a11y(&mut input);
    assert_eq!(input.a11y_status, no_content());
}

#[test]
fn must_fail_empty_group_and_custom_leaves_are_not_content() {
    let mut input = input_with(vec![
        node(1, 1, "  ", "group", false),
        node(2, 1, "", "custom", false),
    ]);
    mark_sparse_target_a11y(&mut input);
    assert_eq!(input.a11y_status, no_content());
}

#[test]
fn must_pass_named_pane_leaf_is_content() {
    let mut input = input_with(vec![node(1, 1, "Editor", "pane", false)]);
    mark_sparse_target_a11y(&mut input);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

#[test]
fn must_pass_empty_pane_with_children_is_content() {
    let mut pane = node(1, 1, "", "pane", false);
    pane.children_count = 1;
    let mut input = input_with(vec![pane, node(2, 2, "Search", "edit", false)]);
    mark_sparse_target_a11y(&mut input);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

#[test]
fn must_pass_unlabeled_edit_leaf_is_content() {
    let mut input = input_with(vec![node(1, 1, "", "edit", false)]);
    mark_sparse_target_a11y(&mut input);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

#[test]
fn must_pass_empty_pane_with_a_value_is_content() {
    let mut pane = node(1, 1, "", "pane", false);
    pane.value = Some("Now playing".to_owned());
    let mut input = input_with(vec![pane]);
    mark_sparse_target_a11y(&mut input);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

#[test]
fn must_pass_empty_custom_control_with_an_interaction_pattern_is_content() {
    let mut custom = node(1, 1, "", "custom", false);
    custom.patterns = vec![UiaPattern::LegacyIAccessible, UiaPattern::Value];
    let mut input = input_with(vec![custom]);
    mark_sparse_target_a11y(&mut input);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

/// Drives the real `find` matcher over an input whose sparse-UIA verdict is
/// `reason_code` and returns what `find` reports as its coverage.
fn find_coverage_for(reason_code: &str) -> Option<String> {
    let mut input = input_with(vec![node(1, 1, "", "pane", false)]);
    input.a11y_status = SensorStatus::DegradedSensorFailed {
        reason_code: reason_code.to_owned(),
    };
    crate::m1::match_find_input(&input, &crate::m1::FindParams::default()).a11y_coverage
}

#[test]
fn must_fail_no_uia_content_is_a_coverage_reason_that_find_reports() {
    assert_eq!(
        find_coverage_for(A11Y_TARGET_WINDOW_NO_UIA_CONTENT).as_deref(),
        Some(A11Y_TARGET_WINDOW_NO_UIA_CONTENT)
    );
}

#[test]
fn must_fail_minimized_target_without_uia_content_is_a_coverage_reason() {
    assert_eq!(
        find_coverage_for(A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE).as_deref(),
        Some(A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE)
    );
}

#[test]
fn must_pass_an_attached_cdp_tree_supersedes_a_sparse_uia_verdict() {
    assert!(cdp_tree_supersedes_sparse_uia(
        A11Y_TARGET_WINDOW_NO_UIA_CONTENT
    ));
    assert!(cdp_tree_supersedes_sparse_uia(
        A11Y_TARGET_WINDOW_WEBVIEW_CONTENT_INCOMPLETE
    ));
    assert!(!cdp_tree_supersedes_sparse_uia(
        A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED
    ));
}
