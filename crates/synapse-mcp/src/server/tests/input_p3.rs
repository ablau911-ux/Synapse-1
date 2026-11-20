//! Regression tests for #62 (input/act P3 batch).

use super::*;

fn scroll_params(value: serde_json::Value) -> ActScrollParams {
    match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => panic!("test params must deserialize: {error}"),
    }
}

#[test]
fn must_fail_scroll_without_point_or_element_has_no_explicit_claim_target() {
    let params = scroll_params(json!({ "dy": -3 }));
    assert!(scroll_claim_target(&params).is_none());
}

#[test]
fn must_pass_scroll_at_a_point_scopes_the_claim_to_the_window_under_it() {
    // Expected value read from the same production hit-test the router uses:
    // the claim target is exactly the root window under the scroll point.
    for (x, y) in [(0, 0), (10, 10), (200, 150)] {
        let expected = crate::m2::root_hwnd_at_point(synapse_core::Point { x, y })
            .map(super::super::target_claims::window_session_target);
        let params = scroll_params(json!({ "dy": 1, "at": { "x": x, "y": y } }));
        assert_eq!(
            scroll_claim_target(&params),
            expected,
            "scroll at ({x},{y}) must claim-check the window under the point"
        );
    }
}

#[test]
fn must_fail_off_screen_point_has_no_claim_target() {
    let off_screen = scroll_params(json!({ "dy": 1, "at": { "x": -2_000_000, "y": -2_000_000 } }));
    assert_eq!(
        crate::m2::root_hwnd_at_point(synapse_core::Point {
            x: -2_000_000,
            y: -2_000_000
        }),
        None,
        "precondition: nothing is under an off-screen point"
    );
    assert_eq!(scroll_claim_target(&off_screen), None);
}

#[test]
fn must_fail_observe_cdp_gather_cap_bounds_a_huge_element_offset() {
    let requested = 100_000_usize.saturating_add(500);
    let capped = requested
        .max(crate::m1::find_cdp_max_nodes())
        .min(crate::m1::MAX_OBSERVE_CDP_GATHER_NODES);
    assert_eq!(capped, crate::m1::MAX_OBSERVE_CDP_GATHER_NODES);
    const { assert!(crate::m1::MAX_OBSERVE_CDP_GATHER_NODES >= crate::m1::find_cdp_max_nodes()) };
    const { assert!(crate::m1::MAX_OBSERVE_CDP_GATHER_NODES < 100_000) };
}
