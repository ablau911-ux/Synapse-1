//! #107 regression: claimed native point click while OS foreground is another window.

use super::{
    ClaimedPointTarget, PointClickRoute, execute_claimed_point_postmessage_click, point_click_route,
};
use crate::m2::click::click_error_code;
use crate::m2::click::hwnd_post::{
    ChildHwndIdentity, PostedMouseStep, posted_click_steps, revalidate_child_identity,
};
use crate::m2::click::schema::{
    ActClickParams, ActClickPointTarget, ActClickTarget, ClickModifier, default_click_backend,
    default_click_button, default_click_count, default_click_duration_ms,
    default_click_velocity_profile, default_coordinate_fallback_on_unsupported,
    default_use_invoke_pattern,
};
use synapse_core::{Backend, error_codes};

pub(super) fn point_params(backend: Backend, claimed_root: Option<i64>) -> ActClickParams {
    ActClickParams {
        target: ActClickTarget::Point(ActClickPointTarget { x: 12, y: 34 }),
        button: default_click_button(),
        clicks: default_click_count(),
        modifiers: Vec::new(),
        velocity_profile: default_click_velocity_profile(),
        duration_ms: default_click_duration_ms(),
        hold_ms: crate::m2::click::schema::default_click_hold_ms(),
        backend,
        use_invoke_pattern: default_use_invoke_pattern(),
        coordinate_fallback_on_unsupported: default_coordinate_fallback_on_unsupported(),
        verify_delta: true,
        verify_target_window_hwnd: claimed_root,
        verify_timeout_ms: crate::m2::click::schema::default_verify_timeout_ms(),
        auto_wait: false,
        auto_wait_timeout_ms: crate::m2::default_auto_wait_timeout_ms(),
        deprecated_curve_alias_used: false,
        coordinate_first: false,
    }
}

pub(super) const fn claim(root_hwnd: i64) -> ClaimedPointTarget {
    ClaimedPointTarget::from_session_claim(root_hwnd, 7)
}

#[test]
fn must_pass_claimed_point_with_foreground_elsewhere_routes_postmessage() {
    let params = point_params(default_click_backend(), Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), Some(0x2000), Some(claim(0x1000))),
        PointClickRoute::PostMessage(claim(0x1000))
    );
}

#[test]
fn must_pass_claimed_point_software_backend_routes_postmessage() {
    let params = point_params(Backend::Software, Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), Some(0x2000), Some(claim(0x1000))),
        PointClickRoute::PostMessage(claim(0x1000))
    );
}

#[test]
fn must_fail_claimed_point_hardware_backend_refuses_foreground_change() {
    let params = point_params(Backend::Hardware, Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), Some(0x2000), Some(claim(0x1000))),
        PointClickRoute::RefuseForeground
    );
}

#[test]
fn must_pass_claimed_point_already_foreground_stays_direct() {
    let params = point_params(default_click_backend(), Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), Some(0x1000), Some(claim(0x1000))),
        PointClickRoute::Direct
    );
}

#[test]
fn must_pass_point_without_target_window_keeps_direct_route() {
    let params = point_params(default_click_backend(), None);
    assert_eq!(
        point_click_route(&params, None, Some(0x2000), None),
        PointClickRoute::Direct
    );
}

/// Codex review CRITICAL: a caller-supplied verify_target_window_hwnd alone must
/// never qualify a background window for PostMessage delivery.
#[test]
fn must_fail_caller_supplied_hwnd_without_session_claim_is_refused() {
    let params = point_params(default_click_backend(), Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), Some(0x2000), None),
        PointClickRoute::RefuseUnclaimed
    );
}

#[test]
fn must_fail_session_claim_on_a_different_root_is_refused() {
    let params = point_params(default_click_backend(), Some(0x1000));
    assert_eq!(
        point_click_route(&params, Some(0x1000), Some(0x2000), Some(claim(0x3000))),
        PointClickRoute::RefuseUnclaimed
    );
}

#[tokio::test]
async fn must_fail_claimed_point_postmessage_refuses_modifiers_before_delivery() {
    let mut params = point_params(default_click_backend(), Some(0x1000));
    params.modifiers = vec![ClickModifier::Ctrl];
    let error =
        match execute_claimed_point_postmessage_click(&params, claim(0x1000), Vec::new()).await {
            Ok(response) => panic!("modifier PostMessage click must be refused, got {response:?}"),
            Err(error) => error,
        };
    assert_eq!(
        click_error_code(&error),
        error_codes::ACTION_BACKEND_UNAVAILABLE
    );
}

fn identity() -> ChildHwndIdentity {
    ChildHwndIdentity {
        hwnd: 0x2222,
        root_hwnd: 0x1000,
        descends_from_root: true,
        pid: 4242,
        generation: 3,
        root_generation: 1,
    }
}

#[test]
fn must_pass_unchanged_child_identity_revalidates() {
    let expected = identity();
    assert!(revalidate_child_identity(&expected, Some(&identity())).is_ok());
}

/// The refusal must be the typed stale-target error, and its detail must name
/// the specific identity check that failed.
fn stale_detail(observed: Option<&ChildHwndIdentity>) -> String {
    match revalidate_child_identity(&identity(), observed) {
        Err(synapse_action::ActionError::TargetInvalid { detail }) => {
            assert!(
                detail.contains("child HWND 0x2222 under claimed root 0x1000 is stale"),
                "{detail}"
            );
            assert!(
                detail.contains("no further mouse messages were posted"),
                "{detail}"
            );
            detail
        }
        other => panic!("expected ActionError::TargetInvalid, got {other:?}"),
    }
}

#[test]
fn must_fail_destroyed_child_is_refused() {
    let detail = stale_detail(None);
    assert!(detail.contains("no longer exists"), "{detail}");
}

#[test]
fn must_fail_recycled_child_hwnd_is_refused() {
    let observed = ChildHwndIdentity {
        generation: 4,
        ..identity()
    };
    let detail = stale_detail(Some(&observed));
    assert!(detail.contains("generation changed"), "{detail}");
    assert!(detail.contains("child 3->4"), "{detail}");
}

#[test]
fn must_fail_reparented_child_is_refused() {
    let observed = ChildHwndIdentity {
        descends_from_root: false,
        ..identity()
    };
    let detail = stale_detail(Some(&observed));
    assert!(detail.contains("no longer a descendant"), "{detail}");
}

#[test]
fn must_fail_child_owned_by_another_process_is_refused() {
    let observed = ChildHwndIdentity {
        pid: 9999,
        ..identity()
    };
    let detail = stale_detail(Some(&observed));
    assert!(
        detail.contains("owner pid changed from 4242 to 9999"),
        "{detail}"
    );
}

#[test]
fn must_fail_recycled_root_is_refused() {
    let observed = ChildHwndIdentity {
        root_generation: 2,
        ..identity()
    };
    let detail = stale_detail(Some(&observed));
    assert!(detail.contains("generation changed"), "{detail}");
    assert!(detail.contains("root 1->2"), "{detail}");
}

#[test]
fn must_fail_child_or_root_hwnd_mismatch_is_refused() {
    let other_child = ChildHwndIdentity {
        hwnd: 0x3333,
        ..identity()
    };
    let detail = stale_detail(Some(&other_child));
    assert!(detail.contains("no longer a descendant"), "{detail}");

    let other_root = ChildHwndIdentity {
        root_hwnd: 0x4444,
        ..identity()
    };
    let detail = stale_detail(Some(&other_root));
    assert!(detail.contains("no longer a descendant"), "{detail}");
}

#[test]
fn must_pass_double_click_posts_down_up_dblclk_up() {
    use PostedMouseStep::{DoubleClick, Down, Hold, InterClickDelay, Move, Up};
    assert_eq!(
        posted_click_steps(2, true),
        vec![Move, Down, Hold, Up, InterClickDelay, DoubleClick, Hold, Up]
    );
}

#[test]
fn must_pass_triple_click_third_press_is_plain_down() {
    use PostedMouseStep::{DoubleClick, Down, Hold, InterClickDelay, Move, Up};
    assert_eq!(
        posted_click_steps(3, true),
        vec![
            Move,
            Down,
            Hold,
            Up,
            InterClickDelay,
            DoubleClick,
            Hold,
            Up,
            InterClickDelay,
            Down,
            Hold,
            Up,
        ]
    );
}

#[test]
fn must_pass_class_without_cs_dblclks_gets_plain_downs() {
    use PostedMouseStep::{Down, Hold, InterClickDelay, Move, Up};
    assert_eq!(
        posted_click_steps(2, false),
        vec![Move, Down, Hold, Up, InterClickDelay, Down, Hold, Up]
    );
}

#[test]
fn must_pass_single_click_posts_one_down_up() {
    use PostedMouseStep::{Down, Hold, Move, Up};
    assert_eq!(posted_click_steps(1, true), vec![Move, Down, Hold, Up]);
}
