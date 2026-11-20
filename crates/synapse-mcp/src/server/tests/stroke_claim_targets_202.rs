//! #202: `act_stroke` clears the claim of every window its planned path lands
//! in, not only the caller's own session target. When the stroke is routed into
//! the session's CDP tab (page coordinates, not screen pixels), only that tab is
//! checked: the route is decided once and drives both the claim check and the
//! dispatch.

use super::{ActStrokeRoute, SessionTarget, act_stroke_route, stroke_claim_targets};

fn line_stroke(from_x: f64, to_x: f64) -> crate::m2::ActStrokePlan {
    let params: crate::m2::ActStrokeParams = serde_json::from_value(serde_json::json!({
        "path": {"kind": "line", "from": {"x": from_x, "y": 10.0}, "to": {"x": to_x, "y": 10.0}},
        "duration_or_speed": {"kind": "duration_ms", "duration_ms": 100}
    }))
    .unwrap_or_else(|error| panic!("stroke params: {error}"));
    crate::m2::validate_act_stroke_params(&params)
        .unwrap_or_else(|error| panic!("stroke plan: {error:?}"))
}

/// Fake desktop: x < 50 is window 0xA (the caller's), x >= 50 is window 0xB (a peer's).
fn root_at(point: synapse_core::Point) -> Option<i64> {
    Some(if point.x < 50 { 0xA } else { 0xB })
}

fn session_tab() -> SessionTarget {
    SessionTarget::Cdp {
        window_hwnd: 0xC,
        cdp_target_id: "TAB-T".to_owned(),
    }
}

fn screen_targets(plan: &crate::m2::ActStrokePlan) -> Vec<SessionTarget> {
    stroke_claim_targets(&ActStrokeRoute::Screen, plan, root_at)
}

#[test]
fn must_fail_a_drag_into_a_peers_window_is_checked_against_that_window() {
    let targets = screen_targets(&line_stroke(10.0, 90.0));
    // Exactly the two windows under the path, sorted, no duplicates, nothing else.
    assert_eq!(
        targets,
        vec![
            SessionTarget::Window { hwnd: 0xA },
            SessionTarget::Window { hwnd: 0xB },
        ],
        "the peer window under the path must be checked, once"
    );
}

#[test]
fn must_pass_a_stroke_inside_one_window_checks_only_that_window() {
    let targets = screen_targets(&line_stroke(10.0, 40.0));
    assert_eq!(targets, vec![SessionTarget::Window { hwnd: 0xA }]);
}

#[test]
fn must_fail_a_stroke_routed_to_the_session_cdp_tab_is_checked_against_that_tab() {
    let plan = line_stroke(10.0, 90.0);
    let route = act_stroke_route(Some(session_tab()), &plan, false);
    assert_eq!(
        route,
        ActStrokeRoute::CdpTarget {
            window_hwnd: 0xC,
            cdp_target_id: "TAB-T".to_owned(),
        },
        "an explicit path with a CDP session target dispatches into that tab"
    );
    let targets = stroke_claim_targets(&route, &plan, root_at);
    assert_eq!(
        targets,
        vec![session_tab()],
        "page coordinates must not be resolved against screen windows"
    );
}

#[test]
fn must_pass_a_screen_stroke_still_checks_every_window_under_its_path() {
    let plan = line_stroke(10.0, 90.0);
    let route = act_stroke_route(Some(SessionTarget::Window { hwnd: 0xA }), &plan, false);
    assert_eq!(route, ActStrokeRoute::Screen);
    let targets = stroke_claim_targets(&route, &plan, root_at);
    assert!(targets.contains(&SessionTarget::Window { hwnd: 0xA }));
    assert!(targets.contains(&SessionTarget::Window { hwnd: 0xB }));
}

#[test]
fn must_pass_no_session_target_routes_to_the_screen() {
    let plan = line_stroke(10.0, 90.0);
    assert_eq!(act_stroke_route(None, &plan, false), ActStrokeRoute::Screen);
}

#[test]
fn must_fail_forced_screen_input_never_takes_the_cdp_route() {
    // Recording or the hardware backend sends real screen input even when the
    // session target is a tab, so the windows under the path are checked.
    let plan = line_stroke(10.0, 90.0);
    let route = act_stroke_route(Some(session_tab()), &plan, true);
    assert_eq!(route, ActStrokeRoute::Screen);
    let targets = stroke_claim_targets(&route, &plan, root_at);
    assert!(targets.contains(&SessionTarget::Window { hwnd: 0xB }));
    assert!(!targets.contains(&session_tab()));
}
