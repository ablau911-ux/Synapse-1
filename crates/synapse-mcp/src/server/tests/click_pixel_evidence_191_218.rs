//! #191: pixel evidence counts only when both captures succeeded; a capture
//! that failed on one side is "no evidence", never an observed delta.
//! #218: when focus moved and no element changed state, a pixel change (focus
//! ring, default-button border, caret) cannot prove a click effect.

use super::*;

fn baseline() -> ClickDeltaSignature {
    tests::click_signature(0x100, 10, "app.exe", "App", 4)
}

fn capture_failed(mut signature: ClickDeltaSignature) -> ClickDeltaSignature {
    signature.pixel.status = "unavailable".to_owned();
    signature.pixel.bitmap_sha256 = None;
    signature.pixel.detail = Some("window capture failed".to_owned());
    signature
}

fn focus_and_pixels_moved(mut signature: ClickDeltaSignature) -> ClickDeltaSignature {
    signature.focused_element_id = Some("focused.other".to_owned());
    signature.focused_bbox = Some(Rect {
        x: 50,
        y: 60,
        w: 80,
        h: 24,
    });
    signature.elements_sha256 = "elements-focus-flag-moved".to_owned();
    signature.pixel.bitmap_sha256 = Some("focus-ring-drawn".to_owned());
    signature
}

#[test]
fn must_fail_capture_failing_on_one_side_is_not_a_delta() {
    let before = baseline();
    let after = capture_failed(baseline());
    assert!(!click_effect_changed(&before, &after));
    assert!(!click_effect::observed_state_changed(&before, &after));
    assert!(!click_effect_changed(&after, &before));
    assert!(!click_effect::observed_state_changed(&after, &before));
}

#[test]
fn must_fail_point_pixel_missing_on_one_side_is_not_a_delta() {
    let before = baseline();
    let mut after = baseline();
    after.point_pixel = Some(after.pixel.clone());
    assert!(!click_effect_changed(&before, &after));
    assert!(!click_effect::observed_state_changed(&before, &after));
}

#[test]
fn must_pass_successful_captures_that_differ_are_a_delta() {
    let before = baseline();
    let mut after = baseline();
    after.pixel.bitmap_sha256 = Some("pixel-changed".to_owned());
    assert!(click_effect_changed(&before, &after));
    assert!(click_effect::observed_state_changed(&before, &after));
}

#[test]
fn must_fail_focus_move_with_pixel_change_only_is_not_a_click_effect() {
    let before = baseline();
    let after = focus_and_pixels_moved(baseline());
    assert!(!click_effect_changed(&before, &after));
}

#[test]
fn must_pass_focus_move_with_a_real_element_change_is_a_click_effect() {
    let before = baseline();
    let mut after = focus_and_pixels_moved(baseline());
    after.effect_elements_sha256 = "toggle-state-changed".to_owned();
    assert!(click_effect_changed(&before, &after));
}

fn point(status: &str, hash: Option<&str>) -> ClickPixelSignature {
    let mut pixel = baseline().pixel;
    pixel.status = status.to_owned();
    pixel.bitmap_sha256 = hash.map(ToOwned::to_owned);
    pixel
}

#[test]
fn must_fail_point_pixels_with_a_failed_or_missing_capture_are_unavailable_evidence() {
    use click_effect::{PointPixelEvidence, point_pixel_evidence};
    let ok = point("ok", Some("before"));
    let failed = point("unavailable", None);
    assert_eq!(
        point_pixel_evidence(Some(&ok), Some(&failed)),
        PointPixelEvidence::Unavailable
    );
    assert_eq!(
        point_pixel_evidence(Some(&failed), Some(&ok)),
        PointPixelEvidence::Unavailable
    );
    assert_eq!(
        point_pixel_evidence(Some(&ok), None),
        PointPixelEvidence::Unavailable
    );
    // Two failures with different error details are still no evidence.
    let mut failed_other = failed.clone();
    failed_other.detail = Some("another capture error".to_owned());
    assert_eq!(
        point_pixel_evidence(Some(&failed), Some(&failed_other)),
        PointPixelEvidence::Unavailable
    );
}

#[test]
fn must_pass_point_pixels_from_two_successful_captures_are_compared() {
    use click_effect::{PointPixelEvidence, point_pixel_evidence};
    let before = point("ok", Some("before"));
    assert_eq!(
        point_pixel_evidence(Some(&before), Some(&point("ok", Some("after")))),
        PointPixelEvidence::Changed
    );
    assert_eq!(
        point_pixel_evidence(Some(&before), Some(&before.clone())),
        PointPixelEvidence::Unchanged
    );
}
