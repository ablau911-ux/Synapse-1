//! Frozen behaviour table for #300 row 300a: which tier `target_act` check / uncheck /
//! click uses for a given session target and locator.
//!
//! Written by the manager from the behaviour table in issue #300 (work method v3).
//! Code agents must not edit this file. Deviation logged in #300: written after the
//! code; red evidence is the red run on 1cc289c9 (every native row went to
//! `chrome_debugger_bridge.domAction`).

use serde_json::json;
use synapse_core::ElementId;

use super::{
    NativePatternAction, NativePatternLocator, TargetActParams, target_act_native_pattern_route,
};
use crate::server::SessionTarget;

const HWND: i64 = 0x1234;
const NATIVE_ID: &str = "0x1234:2a0001";

fn params(value: serde_json::Value) -> TargetActParams {
    match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => panic!("table setup: params do not deserialize: {error}"),
    }
}

fn native_id() -> ElementId {
    match ElementId::parse(NATIVE_ID) {
        Ok(id) => id,
        Err(error) => panic!("table setup: element id: {error:?}"),
    }
}

#[test]
fn native_route_table() {
    let window = SessionTarget::Window { hwnd: HWND };
    let cdp = SessionTarget::Cdp {
        window_hwnd: HWND,
        cdp_target_id: "T1".to_owned(),
    };

    // normal: native window + check by element id -> UIA toggle on
    let p = params(json!({"verb": "check", "element_id": NATIVE_ID}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        Some((
            HWND,
            NativePatternAction::SetChecked(true),
            NativePatternLocator::ElementId(native_id())
        )),
        "row window_check_element_id"
    );

    // uncheck -> UIA toggle off
    let p = params(json!({"verb": "uncheck", "element_id": NATIVE_ID}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        Some((
            HWND,
            NativePatternAction::SetChecked(false),
            NativePatternLocator::ElementId(native_id())
        )),
        "row window_uncheck_element_id"
    );

    // click by name + role -> the UIA pattern chain (manager 2026-11-19, #107 part b:
    // the action is renamed from Invoke to Click; Given and meaning unchanged, logged in #300)
    let p = params(json!({"verb": "click", "name": "Press Me", "role": "button"}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        Some((
            HWND,
            NativePatternAction::Click,
            NativePatternLocator::NameAndRole {
                name: "Press Me",
                role: "button"
            }
        )),
        "row window_click_name_role"
    );

    // #107 part b, row N2: a radio button by name + role takes the same route
    let p = params(json!({"verb": "click", "name": "Option B", "role": "radio button"}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        Some((
            HWND,
            NativePatternAction::Click,
            NativePatternLocator::NameAndRole {
                name: "Option B",
                role: "radio button"
            }
        )),
        "row window_click_radio_name_role"
    );

    // boundary: blank element id falls through to name + role
    let p = params(
        json!({"verb": "check", "element_id": "  ", "name": "Bench option", "role": "checkbox"}),
    );
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        Some((
            HWND,
            NativePatternAction::SetChecked(true),
            NativePatternLocator::NameAndRole {
                name: "Bench option",
                role: "checkbox"
            }
        )),
        "row blank_element_id_uses_name_role"
    );

    // neighbour: a CDP tab target keeps the DOM route
    let p = params(json!({"verb": "check", "element_id": NATIVE_ID}));
    assert_eq!(
        target_act_native_pattern_route(Some(&cdp), &p),
        None,
        "row cdp_target_dom_route"
    );

    // empty: no session target -> not the native route (today's refusal stays)
    assert_eq!(
        target_act_native_pattern_route(None, &p),
        None,
        "row no_target"
    );

    // a CSS selector is a DOM locator, never the native route
    let p = params(json!({"verb": "check", "selector": "#box"}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        None,
        "row selector_is_dom"
    );

    // structurally incomplete: name without role, or nothing at all
    let p = params(json!({"verb": "click", "name": "Press Me"}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        None,
        "row name_without_role"
    );
    let p = params(json!({"verb": "click"}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        None,
        "row no_locator"
    );

    // other verbs never take this route
    let p = params(json!({"verb": "focus", "element_id": NATIVE_ID}));
    assert_eq!(
        target_act_native_pattern_route(Some(&window), &p),
        None,
        "row other_verb"
    );
}
