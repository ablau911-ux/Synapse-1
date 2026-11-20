//! #7 regression: a session-claimed background point click must route to
//! PostMessage when the OS has no foreground window at all (locked screen,
//! desktop focus). Unclaimed / hardware / targetless requests keep refusing.

use super::tests_107::{claim, point_params};
use super::{PointClickRoute, point_click_route};
use synapse_core::Backend;

#[test]
fn must_pass_claimed_point_with_no_foreground_routes_postmessage() {
    let params = point_params(Backend::Auto, Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), None, Some(claim(0x1000))),
        PointClickRoute::PostMessage(claim(0x1000))
    );
}

#[test]
fn must_fail_unclaimed_point_with_no_foreground_is_refused() {
    let params = point_params(Backend::Auto, Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), None, None),
        PointClickRoute::RefuseUnclaimed
    );
}

#[test]
fn must_fail_hardware_point_with_no_foreground_refuses_foreground_change() {
    let params = point_params(Backend::Hardware, Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), None, Some(claim(0x1000))),
        PointClickRoute::RefuseForeground
    );
}
