//! #77: the Chromium renderer UIA supplement drops nodes past its cap or on a
//! node read error; when it reports that, the coverage verdict must degrade.

use super::*;
use crate::m1::cdp_coverage::apply_renderer_supplement_coverage;
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
fn must_fail_truncated_supplement_degrades_a_healthy_status() {
    let mut input = input(SensorStatus::Healthy);
    apply_renderer_supplement_coverage(&mut input, true);
    assert_eq!(input.a11y_status, truncated());
}

#[test]
fn must_pass_complete_supplement_leaves_status_healthy() {
    let mut input = input(SensorStatus::Healthy);
    apply_renderer_supplement_coverage(&mut input, false);
    assert_eq!(input.a11y_status, SensorStatus::Healthy);
}

#[test]
fn must_fail_existing_degraded_reason_is_kept() {
    let other = SensorStatus::DegradedSensorFailed {
        reason_code: "A11Y_TARGET_WINDOW_WEBVIEW_CONTENT_INCOMPLETE".to_owned(),
    };
    let mut input = input(other.clone());
    apply_renderer_supplement_coverage(&mut input, true);
    assert_eq!(input.a11y_status, other);
}
