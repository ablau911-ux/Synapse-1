//! #153: the real action gate (`SynapseService::ensure_supported_use_allows_action_for`,
//! the call `act_click`/`act_type` make with their element id) must admit a
//! CDP web-node click/type when the OS has no foreground window, and keep
//! every other combination fail-closed with `A11Y_NO_FOREGROUND`.
//!
//! Contract: server/action_preflight.rs `action_requires_live_foreground` doc;
//! server/context.rs `ensure_supported_use_allows_action_scoped` (#1061 degraded
//! path returns the `no_foreground_scope_evaluated` preflight). The existing
//! cdp_no_foreground_153.rs tests only the predicate; this drives the gate.

use synapse_core::{ElementId, error_codes};

use crate::server::g4_unit_support::{error_code, force_no_foreground, isolated_service};

const HWND: i64 = 0x0001_5300;

fn cdp_id() -> ElementId {
    synapse_a11y::cdp_element_id(HWND, 42)
}

fn native_id() -> ElementId {
    ElementId::parse("0x15300:0000002a00000001").expect("valid native id")
}

fn gate_error(
    service: &crate::server::SynapseService,
    tool: &'static str,
    element: Option<&ElementId>,
) -> Option<String> {
    service
        .ensure_supported_use_allows_action_for(tool, element)
        .err()
        .as_ref()
        .and_then(error_code)
}

#[test]
fn must_pass_cdp_element_click_and_type_pass_the_gate_with_no_foreground() {
    let (_root, service) = isolated_service();
    assert_eq!(
        service.allow_unknown_profile().ok(),
        Some(true),
        "precondition"
    );
    force_no_foreground(&service);
    for tool in ["act_click", "act_type"] {
        let preflight = service
            .ensure_supported_use_allows_action_for(tool, Some(&cdp_id()))
            .unwrap_or_else(|error| panic!("{tool} CDP element must pass: {error:?}"));
        assert_eq!(preflight.tool, tool);
        assert_eq!(preflight.status, "no_foreground_scope_evaluated");
    }
}

#[test]
fn must_fail_native_or_missing_element_click_stays_fail_closed() {
    let (_root, service) = isolated_service();
    force_no_foreground(&service);
    assert_eq!(
        gate_error(&service, "act_click", Some(&native_id())).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
    assert_eq!(
        gate_error(&service, "act_click", None).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
    assert_eq!(
        gate_error(&service, "act_type", None).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
}

#[test]
fn must_fail_raw_input_tools_keep_the_gate_even_with_a_cdp_id() {
    let (_root, service) = isolated_service();
    force_no_foreground(&service);
    for tool in ["act_press", "act_stroke", "act_scroll"] {
        assert_eq!(
            gate_error(&service, tool, Some(&cdp_id())).as_deref(),
            Some(error_codes::A11Y_NO_FOREGROUND),
            "{tool}"
        );
    }
}
