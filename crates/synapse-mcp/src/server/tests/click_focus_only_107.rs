//! #107 regression: a click whose only observable effect is focus moving is not
//! a verified delta. The click signature carries focus fields (focused element,
//! per-element `focused` flag), so a background click that did nothing used to be
//! reported verified whenever focus happened to shift. Focus-only change must read
//! as "no observed delta"; real changes (element state/value, window pixels, tree
//! structure) must still verify.
//!
//! must_pass: real effects (toggle/selection, window pixels, structure) verify.
//! must_fail: focus-only moves, lost focus and identical signatures are refused
//! as click effects.

use super::*;

fn baseline() -> ClickDeltaSignature {
    tests::click_signature(0x100, 10, "app.exe", "App", 4)
}

/// Same signature with only focus moved: different focused element, and the
/// per-element `focused` flag changes the full element hash. The focus-free
/// element hash is unchanged.
fn focus_moved(mut signature: ClickDeltaSignature) -> ClickDeltaSignature {
    signature.focused_element_id = Some("focused.other".to_owned());
    signature.focused_role = Some("Button".to_owned());
    signature.focused_name_sha256 = non_empty_sha256("other focus");
    signature.focused_value_sha256 = None;
    signature.focused_bbox = Some(Rect {
        x: 50,
        y: 60,
        w: 80,
        h: 24,
    });
    signature.elements_sha256 = "elements-focus-flag-moved".to_owned();
    signature
}

#[test]
fn must_fail_focus_only_change_is_not_a_click_effect() {
    let before = baseline();
    let after = focus_moved(baseline());
    assert_ne!(before, after, "fixture must differ in focus fields");
    assert!(!click_effect_changed(&before, &after));
}

#[test]
fn must_fail_losing_focus_entirely_is_not_a_click_effect() {
    let before = baseline();
    let mut after = baseline();
    after.focused_element_id = None;
    after.focused_role = None;
    after.focused_name_sha256 = None;
    after.focused_value_sha256 = None;
    after.focused_bbox = None;
    assert!(!click_effect_changed(&before, &after));
}

#[test]
fn must_pass_toggle_or_selection_state_change_is_a_click_effect() {
    // A toggled/selected element changes the focus-free element fingerprint.
    let before = baseline();
    let mut after = baseline();
    after.effect_elements_sha256 = "elements-toggle-state-changed".to_owned();
    assert!(click_effect_changed(&before, &after));
}

#[test]
fn must_pass_toggle_change_still_verifies_when_focus_also_moves() {
    let before = baseline();
    let mut after = focus_moved(baseline());
    after.effect_elements_sha256 = "elements-toggle-state-changed".to_owned();
    assert!(click_effect_changed(&before, &after));
}

#[test]
fn must_pass_target_window_pixel_change_is_a_click_effect() {
    let before = baseline();
    let mut after = baseline();
    after.pixel.bitmap_sha256 = Some("pixel-signature-after".to_owned());
    assert!(click_effect_changed(&before, &after));
}

#[test]
fn must_pass_structure_change_is_a_click_effect() {
    let before = baseline();
    let mut after = baseline();
    after.element_count += 1;
    assert!(click_effect_changed(&before, &after));
}

#[test]
fn must_fail_identical_signatures_are_not_a_click_effect() {
    assert!(!click_effect_changed(&baseline(), &baseline()));
}

#[test]
fn must_pass_no_delta_readback_flags_a_focus_only_change() {
    let before = baseline();
    let after = focus_moved(baseline());
    let readback = click_delta_readback(&before, &after, None);
    assert_eq!(readback["focus_only_change"], json!(true));
    let unchanged = click_delta_readback(&before, &before, None);
    assert_eq!(unchanged["focus_only_change"], json!(false));
}
