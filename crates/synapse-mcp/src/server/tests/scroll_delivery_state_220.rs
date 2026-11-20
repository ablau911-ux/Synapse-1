//! #220 regression: `target_act verb=scroll` runs `act_scroll` with
//! `verify_delta=false`, so its postcondition is `not_requested`. The facade must
//! then report `delivered_unverified`, never `delivered_and_verified` or
//! `delivered_and_pixel_verified`: a verified delivery state means a Source-of-
//! Truth delta was actually observed.

use super::*;
use crate::m2::ActScrollResponse;
use crate::m2::postcondition::{postcondition_not_requested, postcondition_observed_delta};

fn scroll_response(postcondition: crate::m2::postcondition::ActPostcondition) -> ActScrollResponse {
    ActScrollResponse {
        ok: true,
        dy: -3,
        dx: 0,
        smooth: false,
        scrolled: true,
        wheel_event_count: 1,
        smooth_interval_ms: 0,
        scheduled_smooth_total_ms: 0,
        backend_used: "software_window_message".to_owned(),
        backend_tier_used: "postmessage".to_owned(),
        required_foreground: false,
        elapsed_ms: 4,
        postcondition,
    }
}

fn delivery_state(response: ActScrollResponse) -> Result<&'static str, ErrorData> {
    let (_, ok, status, result) = target_act_delegate_response("act_scroll", Ok(Json(response)))?;
    Ok(DeliveryClaim::new("scroll", ok, status, &result).state())
}

#[test]
fn must_fail_scroll_without_verify_delta_is_delivered_unverified() -> Result<(), ErrorData> {
    let response = scroll_response(postcondition_not_requested(
        "act_scroll",
        "target_point_pixels_or_foreground_ui",
    ));
    assert_eq!(delivery_state(response)?, "delivered_unverified");
    Ok(())
}

#[test]
fn must_fail_not_requested_postcondition_is_never_pixel_verified() -> Result<(), ErrorData> {
    // The visual-source label must not turn an unrequested postcondition into
    // "delivered_and_pixel_verified".
    let response = scroll_response(postcondition_not_requested(
        "act_scroll",
        "foreground_focused_ui_or_pixels",
    ));
    assert_eq!(delivery_state(response)?, "delivered_unverified");
    Ok(())
}

#[test]
fn must_pass_scroll_with_observed_delta_is_still_verified() -> Result<(), ErrorData> {
    let response = scroll_response(postcondition_observed_delta(
        "act_scroll",
        "target_point_pixels_or_foreground_ui",
        "before".to_owned(),
        "after".to_owned(),
        "observed scroll",
    ));
    assert_eq!(delivery_state(response)?, "delivered_and_verified");
    Ok(())
}
