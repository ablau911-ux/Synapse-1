//! #202: a CDP web element is claimed at its tab; native and legacy
//! backend-only element ids keep the whole-window claim target.

use super::{SessionTarget, press_claim_target, press_claim_targets, web_element_claim_target};

const HWND: i64 = 0x4242;

#[test]
fn must_pass_target_aware_cdp_element_resolves_to_its_tab() {
    let element = synapse_a11y::cdp_element_id_for_target(HWND, "a1b2c3d4", 7);
    assert_eq!(
        web_element_claim_target(&element, HWND),
        SessionTarget::Cdp {
            window_hwnd: HWND,
            cdp_target_id: "A1B2C3D4".to_owned(),
        }
    );
}

#[test]
fn must_fail_a_legacy_backend_only_cdp_element_never_narrows_to_a_tab() {
    let element = synapse_a11y::cdp_element_id(HWND, 7);
    assert_eq!(
        web_element_claim_target(&element, HWND),
        SessionTarget::Window { hwnd: HWND }
    );
}

#[test]
fn must_fail_press_with_a_web_auto_wait_element_is_checked_at_the_whole_window() {
    // Keys go to the dispatch target, which may be a peer's tab in this window,
    // so the claim check must not narrow to the auto-wait element's tab.
    let element = synapse_a11y::cdp_element_id_for_target(HWND, "a1b2c3d4", 7);
    let params: crate::m2::ActPressParams = serde_json::from_value(serde_json::json!({
        "keys": ["Enter"],
        "auto_wait_element_id": element,
    }))
    .unwrap_or_else(|error| panic!("act_press params: {error}"));
    assert_eq!(
        press_claim_target(&params),
        Some(SessionTarget::Window { hwnd: HWND })
    );
}

#[test]
fn must_pass_press_without_an_auto_wait_element_uses_the_session_target() {
    let params: crate::m2::ActPressParams =
        serde_json::from_value(serde_json::json!({ "keys": ["Enter"] }))
            .unwrap_or_else(|error| panic!("act_press params: {error}"));
    assert_eq!(press_claim_target(&params), None);
}

#[test]
fn must_fail_press_with_an_explicit_peer_tab_is_checked_against_that_tab() {
    // Keys dispatched to an explicit window_hwnd + cdp_target_id must be checked
    // against that target, not against the caller's own session target.
    let params: crate::m2::ActPressParams = serde_json::from_value(serde_json::json!({
        "keys": ["Enter"],
        "window_hwnd": HWND,
        "cdp_target_id": "PEER-TAB-T1",
    }))
    .unwrap_or_else(|error| panic!("act_press params: {error}"));
    let (dispatch, element_window) =
        press_claim_targets(&params).unwrap_or_else(|error| panic!("targets: {error:?}"));
    assert_eq!(
        dispatch,
        Some(SessionTarget::Cdp {
            window_hwnd: HWND,
            cdp_target_id: "PEER-TAB-T1".to_owned(),
        })
    );
    assert_eq!(element_window, None);
}

#[test]
fn must_fail_press_with_an_explicit_window_and_web_auto_wait_element_checks_both() {
    let element = synapse_a11y::cdp_element_id_for_target(HWND, "a1b2c3d4", 7);
    let params: crate::m2::ActPressParams = serde_json::from_value(serde_json::json!({
        "keys": ["Enter"],
        "window_hwnd": 0x5151,
        "auto_wait_element_id": element,
    }))
    .unwrap_or_else(|error| panic!("act_press params: {error}"));
    let (dispatch, element_window) =
        press_claim_targets(&params).unwrap_or_else(|error| panic!("targets: {error:?}"));
    assert_eq!(dispatch, Some(SessionTarget::Window { hwnd: 0x5151 }));
    assert_eq!(element_window, Some(SessionTarget::Window { hwnd: HWND }));
}
