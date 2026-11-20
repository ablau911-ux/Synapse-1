//! #189: real post-dispatch producer plus guards for non-injectable callers.
use super::*;

#[cfg(windows)]
#[path = "coordinate_routes_189.rs"]
pub(crate) mod coordinate_routes_189;

pub(crate) fn after_dispatch(error: &ActionError, index: u8, clicks: u8) -> ErrorData {
    let outcomes = vec!["invoked".to_owned(); usize::from(index)];
    uia_click_error_after_dispatch(error, index, clicks, &outcomes).unwrap_or_else(|| {
        attach_click_tier_attempts(
            mark_click_not_dispatched(action_error_to_mcp(error)),
            vec![click_tier_failed(
                CLICK_TIER_UIA,
                click_reason_for_error_code(error.code()),
                error.code(),
                false,
                error.detail(),
            )],
        )
    })
}

pub(crate) fn before_dispatch(error: ErrorData) -> ErrorData {
    mark_click_not_dispatched(error)
}

/// U9: a typed worker admission refusal carries no post-dispatch marker.
#[test]
fn must_pass_first_non_timeout_error_has_no_postdispatch_marker() {
    for phase in ["worker_limit", "worker_busy", "worker_unavailable"] {
        let error = ActionError::ClickNotDispatched {
            source: Box::new(ActionError::BackendUnavailable {
                detail: format!("phase={phase}"),
            }),
            tier_unavailable: true,
        };
        assert!(
            uia_click_error_after_dispatch(&error, 0, 2, &[]).is_none(),
            "#189: pre-send failure is proven undelivered: {phase}"
        );
    }
}

/// U3: an element stale before any pattern is refused before delivery.
#[test]
fn must_fail_stale_before_dispatch_is_refused_without_escalation() {
    let error = after_dispatch(
        &ActionError::ClickNotDispatched {
            source: Box::new(ActionError::TransientElementExpired {
                element_id: synapse_core::element_id(189, "01020304"),
                detail: "re-resolve found no element".to_owned(),
            }),
            tier_unavailable: false,
        },
        0,
        1,
    );
    crate::server::dispatch_status_189_tests::assert_not_delivered(
        &error,
        "refused_before_delivery",
    );
}

/// R5: an untyped error from the dispatching call is unknown delivery, never
/// "not dispatched" and never escalation, whatever its code.
#[test]
fn must_fail_untyped_dispatch_error_defaults_to_possible_delivery() {
    for source in [
        ActionError::BackendUnavailable {
            detail: "phase=worker_busy".to_owned(),
        },
        ActionError::ElementPatternUnsupported {
            element_id: synapse_core::element_id(189, "01020304"),
            detail: "no supported pattern".to_owned(),
        },
        ActionError::TargetInvalid {
            detail: "opaque provider failure".to_owned(),
        },
    ] {
        let error = uia_click_error_after_dispatch(&source, 0, 1, &[])
            .unwrap_or_else(|| panic!("untyped error must not count as undelivered: {source:?}"));
        crate::server::dispatch_status_189_tests::assert_verify_needed(&error);
        let data = error.data.as_ref().unwrap();
        assert_eq!(data["possibly_delivered"], true);
        assert_eq!(data["router_escalation_allowed"], false);
    }
}

#[cfg(windows)]
#[test]
fn must_fail_native_point_refusal_never_scrolls_or_requests_foreground() {
    use std::cell::Cell;
    let id = synapse_core::element_id(189, "01020304");
    let reads = Cell::new(0);
    let bbox = synapse_core::Rect {
        x: 500,
        y: 500,
        w: 20,
        h: 20,
    };
    let result = native_click_point_with(
        &id,
        189,
        false,
        false,
        |_| {
            reads.set(reads.get() + 1);
            Ok(bbox)
        },
        |_| {
            Ok(synapse_core::Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 100,
            })
        },
        |_| panic!("background refusal must not read foreground metadata"),
        |_, _, _, _| panic!("background refusal must not scroll or activate"),
    );
    assert_eq!(reads.get(), 1);
    let error = result
        .err()
        .unwrap_or_else(|| panic!("outside target must refuse"));
    assert_eq!(
        error.data.as_ref().map(|data| &data["reason"]),
        Some(&json!("target_click_point_outside_window"))
    );
}

/// #189 B8 (extension truthfulness): the whole daemon mapping from the
/// extension's wire payload to the click delivery status. A domAction throw
/// before any event reports `not_started` and must be refused before delivery;
/// a payload without `dispatch_stage` proves nothing and stays possibly
/// delivered (verify before retry). Lives here, not next to the bridge tests:
/// the bridge source is also compiled into the native-host binary.
#[test]
fn must_fail_extension_wire_stage_maps_to_click_delivery_status() {
    use crate::chrome_debugger_bridge::dispatch_stage_189_tests::response_error;
    use crate::server::dispatch_status_189_tests::{assert_not_delivered, assert_verify_needed};
    let code = synapse_core::error_codes::CHROME_DOM_ACTION_UNSUPPORTED;
    let not_started = response_error(json!({
        "code": code,
        "detail": "domAction check failed in resolved frame 0: element is not a checkbox",
        "dispatch_stage": "not_started",
    }));
    let refused = crate::m2::remote_bridge_click_error(&not_started);
    assert_not_delivered(&refused, "refused_before_delivery");

    let missing = response_error(json!({
        "code": code,
        "detail": "domAction click failed in resolved frame 0: page threw",
    }));
    let unknown = crate::m2::remote_bridge_click_error(&missing);
    assert_verify_needed(&unknown);
    assert_eq!(unknown.data.as_ref().unwrap()["possibly_delivered"], true);
}
