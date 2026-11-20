//! #107(c) frozen behaviour table (manager-owned, work method v3). Codex and
//! Opus never edit this file. One case per L row of the #107(c) table: a point
//! click that names its target window resolves that root for routing even with
//! `verify_delta=false`, so a claimed background window is posted to, never
//! sent to the foreground tier; the rest of the point route stays as it is.

use super::{ClaimedPointTarget, PointClickRoute, point_click_resolves_root, point_click_route};
use crate::m2::click::schema::{
    ActClickElementTarget, ActClickParams, ActClickPointTarget, ActClickTarget,
    default_click_button, default_click_count, default_click_duration_ms,
    default_click_velocity_profile, default_coordinate_fallback_on_unsupported,
    default_use_invoke_pattern,
};
use synapse_core::{Backend, ElementId};

const ROOT: i64 = 0x0A_0A0A;
const OWNER: i64 = 0x0C_0C0C;

fn params(target: ActClickTarget, verify_delta: bool, window: Option<i64>) -> ActClickParams {
    ActClickParams {
        target,
        button: default_click_button(),
        clicks: default_click_count(),
        modifiers: Vec::new(),
        velocity_profile: default_click_velocity_profile(),
        duration_ms: default_click_duration_ms(),
        hold_ms: crate::m2::click::schema::default_click_hold_ms(),
        backend: Backend::Auto,
        use_invoke_pattern: default_use_invoke_pattern(),
        coordinate_fallback_on_unsupported: default_coordinate_fallback_on_unsupported(),
        verify_delta,
        verify_target_window_hwnd: window,
        verify_timeout_ms: crate::m2::click::schema::default_verify_timeout_ms(),
        auto_wait: false,
        auto_wait_timeout_ms: crate::m2::default_auto_wait_timeout_ms(),
        deprecated_curve_alias_used: false,
        coordinate_first: false,
    }
}

const fn point() -> ActClickTarget {
    ActClickTarget::Point(ActClickPointTarget { x: 12, y: 34 })
}

#[test]
fn point_click_resolves_root_table_107c() {
    let element = ElementId::parse("0x1000:0000002a00000001")
        .map(|element_id| ActClickTarget::Element(ActClickElementTarget { element_id }));
    let Ok(element) = element else {
        panic!("row setup: synthetic element id did not parse");
    };
    // (row, request, expected: the route reads the target root)
    let rows: [(&str, ActClickParams, bool); 5] = [
        (
            "107m_point_verify_delta_false_with_named_window_resolves_root",
            params(point(), false, Some(ROOT)),
            true,
        ),
        (
            "107m_neighbour_point_verify_delta_true_with_named_window_resolves_root",
            params(point(), true, Some(ROOT)),
            true,
        ),
        (
            "107m_empty_bare_point_without_window_stays_direct",
            params(point(), false, None),
            false,
        ),
        (
            "107m_neighbour_bare_point_verify_delta_true_keeps_todays_read",
            params(point(), true, None),
            true,
        ),
        (
            "107m_neighbour_element_target_verify_delta_false_unchanged",
            params(element, false, None),
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (row, request, expected) in rows {
        let got = point_click_resolves_root(&request);
        if got != expected {
            failures.push(format!(
                "row {row}: resolves root {got}, expected {expected}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "#107(c) root rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn point_click_route_neighbours_table_107c() {
    let claim = ClaimedPointTarget::from_session_claim(ROOT, 7);
    let other_claim = ClaimedPointTarget::from_session_claim(OWNER, 9);
    // (row, request, target root, foreground root, this session's claim, expected route)
    type Row = (
        &'static str,
        ActClickParams,
        Option<i64>,
        Option<i64>,
        Option<ClaimedPointTarget>,
        PointClickRoute,
    );
    let rows: [Row; 6] = [
        (
            "107m_claimed_background_root_with_verify_delta_false_is_posted",
            params(point(), false, Some(ROOT)),
            Some(ROOT),
            Some(OWNER),
            Some(claim),
            PointClickRoute::PostMessage(claim),
        ),
        (
            "107j_second_actor_unclaimed_root_is_refused",
            params(point(), false, Some(ROOT)),
            Some(ROOT),
            Some(OWNER),
            None,
            PointClickRoute::RefuseUnclaimed,
        ),
        (
            "107j_someone_elses_claim_on_another_window_never_authorizes",
            params(point(), false, Some(ROOT)),
            Some(ROOT),
            Some(OWNER),
            Some(other_claim),
            PointClickRoute::RefuseUnclaimed,
        ),
        (
            "107k_neighbour_target_is_foreground_stays_direct",
            params(point(), false, Some(ROOT)),
            Some(ROOT),
            Some(ROOT),
            Some(claim),
            PointClickRoute::Direct,
        ),
        (
            "107l_neighbour_hardware_backend_in_background_is_refused",
            ActClickParams {
                backend: Backend::Hardware,
                ..params(point(), false, Some(ROOT))
            },
            Some(ROOT),
            Some(OWNER),
            Some(claim),
            PointClickRoute::RefuseForeground,
        ),
        (
            "107_restart_no_claim_readback_is_refused",
            params(point(), false, Some(ROOT)),
            Some(ROOT),
            None,
            None,
            PointClickRoute::RefuseUnclaimed,
        ),
    ];
    let mut failures = Vec::new();
    for (row, request, target_root, foreground_root, session_claim, expected) in rows {
        let got = point_click_route(&request, target_root, foreground_root, session_claim);
        if got != expected {
            failures.push(format!("row {row}: route {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#107(c) route rows failed:\n{}",
        failures.join("\n")
    );
}
