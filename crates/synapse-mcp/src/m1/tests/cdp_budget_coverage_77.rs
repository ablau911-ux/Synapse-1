//! #77 (re-review): controls beyond the CDP node budget must flip the
//! observe/find coverage verdict instead of staying `Healthy`.

use super::*;
use crate::m1::cdp_coverage::{apply_cdp_budget_coverage, cdp_budget_truncated};
use synapse_core::{Rect, SensorStatus};
use synapse_perception::ObservationInput;

fn input(status: SensorStatus) -> ObservationInput {
    let mut input = ObservationInput::new(foreground_with_bounds(Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    }));
    input.a11y_status = status;
    input
}

fn truncated() -> SensorStatus {
    SensorStatus::DegradedSensorFailed {
        reason_code: "A11Y_TARGET_WINDOW_SNAPSHOT_TRUNCATED".to_owned(),
    }
}

#[test]
fn must_pass_budget_cut_is_detected_only_when_cap_hit_and_more_nodes_exist() {
    assert!(cdp_budget_truncated(300, 900, 300));
    assert!(cdp_budget_truncated(0, 5, 0));
    // under budget: everything mappable was returned
    assert!(!cdp_budget_truncated(120, 900, 300));
    // exactly at the cap with nothing more on the page
    assert!(!cdp_budget_truncated(300, 300, 300));
    assert!(!cdp_budget_truncated(0, 0, 300));
}

#[test]
fn must_fail_healthy_status_degrades_when_budget_cut() {
    let mut input = input(SensorStatus::Healthy);
    apply_cdp_budget_coverage(&mut input, 300, 900, 300);
    assert_eq!(input.a11y_status, truncated());
}

#[test]
fn must_pass_complete_cdp_tree_leaves_status_healthy() {
    let mut input = input(SensorStatus::Healthy);
    apply_cdp_budget_coverage(&mut input, 50, 80, 300);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

#[test]
fn must_fail_existing_degraded_reason_is_not_overwritten() {
    let other = SensorStatus::DegradedSensorFailed {
        reason_code: "A11Y_TARGET_WINDOW_WEBVIEW_CONTENT_INCOMPLETE".to_owned(),
    };
    let mut input = input(other.clone());
    apply_cdp_budget_coverage(&mut input, 300, 900, 300);
    assert_eq!(input.a11y_status, other);
}
