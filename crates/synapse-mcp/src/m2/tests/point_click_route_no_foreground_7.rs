//! #7: the live point-click route must treat "no OS foreground window at all"
//! (`A11Y_NO_FOREGROUND`) as "target is not foreground": a session-claimed
//! background target still routes to PostMessage, an unclaimed one is refused,
//! and every OTHER foreground read failure fails closed (never Direct).
//!
//! Contract: m2/click/point_post.rs `current_point_click_route` doc + #7
//! "Expected". Existing tests feed `None` straight into the pure
//! `point_click_route`, bypassing the foreground READ this boundary repairs.
//!
//! Seam (NEEDS-SEAM, see G4 report): `pub(crate) fn
//! current_point_click_route_with(params: &ActClickParams, target_root:
//! Option<i64>, claim: Option<ClaimedPointTarget>, foreground_root: impl
//! FnOnce() -> Result<i64, synapse_a11y::A11yError>) -> Result<PointClickRoute,
//! ErrorData>`; `current_point_click_route` must call it with the live
//! `current_foreground_context` + `top_level_root_hwnd` read.

use std::cell::Cell;

use serde_json::{Value, json};

use super::{ActClickParams, ClaimedPointTarget, PointClickRoute, current_point_click_route_with};

const TARGET: i64 = 0x0004_0a10;
const HUMAN: i64 = 0x0004_0b20;

fn point_params(backend: &str) -> ActClickParams {
    serde_json::from_value(json!({
        "target": {"x": 120, "y": 80},
        "backend": backend,
        "verify_delta": true,
        "verify_target_window_hwnd": TARGET,
    }))
    .expect("valid point click params")
}

fn claim() -> ClaimedPointTarget {
    ClaimedPointTarget::from_session_claim(TARGET, 3)
}

fn no_foreground() -> Result<i64, synapse_a11y::A11yError> {
    Err(synapse_a11y::A11yError::NoForeground {
        detail: "GetForegroundWindow returned null".to_owned(),
    })
}

fn code(error: &rmcp::ErrorData) -> Option<String> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[test]
fn must_pass_claimed_target_with_no_os_foreground_routes_postmessage() {
    let route = current_point_click_route_with(
        &point_params("auto"),
        Some(TARGET),
        Some(claim()),
        no_foreground,
    )
    .expect("a missing foreground is a valid routing state");
    assert_eq!(route, PointClickRoute::PostMessage(claim()));
}

#[test]
fn must_pass_claimed_target_behind_a_human_window_routes_postmessage() {
    let route = current_point_click_route_with(
        &point_params("software"),
        Some(TARGET),
        Some(claim()),
        || Ok(HUMAN),
    )
    .expect("route");
    assert_eq!(route, PointClickRoute::PostMessage(claim()));
}

#[test]
fn must_pass_target_that_is_already_foreground_routes_direct() {
    let route =
        current_point_click_route_with(&point_params("auto"), Some(TARGET), None, || Ok(TARGET))
            .expect("route");
    assert_eq!(route, PointClickRoute::Direct);
}

#[test]
fn must_fail_unclaimed_target_with_no_os_foreground_is_refused() {
    let route =
        current_point_click_route_with(&point_params("auto"), Some(TARGET), None, no_foreground)
            .expect("route");
    assert_eq!(route, PointClickRoute::RefuseUnclaimed);
}

#[test]
fn must_fail_hardware_backend_cannot_use_the_background_route() {
    let route = current_point_click_route_with(
        &point_params("hardware"),
        Some(TARGET),
        Some(claim()),
        no_foreground,
    )
    .expect("route");
    assert_eq!(route, PointClickRoute::RefuseForeground);
}

#[test]
fn must_fail_any_other_foreground_read_error_fails_closed() {
    let internal = synapse_a11y::A11yError::internal("foreground probe crashed");
    let expected = internal.code().to_owned();
    let error = current_point_click_route_with(
        &point_params("auto"),
        Some(TARGET),
        Some(claim()),
        move || Err(internal),
    )
    .expect_err("a broken foreground read must not route");
    assert_eq!(code(&error), Some(expected));
}

#[test]
fn must_pass_no_target_window_routes_direct_without_reading_foreground() {
    let reads = Cell::new(0_u32);
    let route = current_point_click_route_with(&point_params("auto"), None, None, || {
        reads.set(reads.get() + 1);
        Ok(HUMAN)
    })
    .expect("route");
    assert_eq!(route, PointClickRoute::Direct);
    assert_eq!(
        reads.get(),
        0,
        "no target window: the foreground is not read"
    );
}
