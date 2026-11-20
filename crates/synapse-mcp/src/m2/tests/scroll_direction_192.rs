//! #192: horizontal scroll direction must be consistent across routes.
//! Convention (Win32): dx > 0 scrolls RIGHT, dy > 0 scrolls UP. SendInput
//! (MOUSEEVENTF_HWHEEL) and PostMessage (WM_MOUSEHWHEEL) pass +120*dx; the CDP route
//! must therefore send a POSITIVE deltaX for dx > 0 (CDP deltaX > 0 = right) and a
//! NEGATIVE deltaY for dy > 0 (CDP deltaY > 0 = down).

use super::{ActScrollParams, WheelPlan, cdp_wheel_deltas};

fn params(dy: i32, dx: i32, smooth: bool) -> ActScrollParams {
    ActScrollParams {
        dy,
        dx,
        smooth,
        ..ActScrollParams::default()
    }
}

#[test]
fn must_pass_cdp_positive_dx_is_positive_delta_x_like_win32_hwheel() {
    let deltas = cdp_wheel_deltas(&params(0, 3, false)).expect("deltas");
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].delta_x, 360.0);
    // Same sign as the Win32 horizontal wheel route for the same input.
    let win32 = WheelPlan::for_notches(3).expect("plan");
    assert_eq!(win32.messages(), [120, 120, 120]);
}

#[test]
fn must_pass_cdp_negative_dx_is_negative_delta_x() {
    let deltas = cdp_wheel_deltas(&params(0, -2, false)).expect("deltas");
    assert_eq!(deltas[0].delta_x, -240.0);
}

#[test]
fn must_pass_cdp_vertical_sign_is_unchanged_positive_dy_is_negative_delta_y() {
    let deltas = cdp_wheel_deltas(&params(2, 0, false)).expect("deltas");
    assert_eq!(deltas[0].delta_y, -240.0);
    let down = cdp_wheel_deltas(&params(-1, 0, false)).expect("deltas");
    assert_eq!(down[0].delta_y, 120.0);
}

#[test]
fn must_pass_cdp_smooth_keeps_the_same_signs_per_step() {
    let deltas = cdp_wheel_deltas(&params(1, 2, true)).expect("deltas");
    assert_eq!(deltas.len(), 2);
    assert!(deltas.iter().all(|d| d.delta_x == 120.0));
    assert_eq!(deltas[0].delta_y, -120.0);
}

#[test]
fn must_fail_zero_scroll_emits_no_wheel_event_on_any_route() {
    let deltas = cdp_wheel_deltas(&params(0, 0, false)).expect("deltas");
    assert!(deltas.is_empty());
    let smooth = cdp_wheel_deltas(&params(0, 0, true)).expect("deltas");
    assert!(smooth.is_empty());
    assert!(
        WheelPlan::for_notches(0)
            .expect("plan")
            .messages()
            .is_empty()
    );
}

#[test]
fn must_fail_horizontal_input_never_leaks_into_the_vertical_axis() {
    let horizontal = cdp_wheel_deltas(&params(0, 3, false)).expect("deltas");
    assert_eq!(horizontal[0].delta_y, 0.0);
    let vertical = cdp_wheel_deltas(&params(-3, 0, false)).expect("deltas");
    assert_eq!(vertical[0].delta_x, 0.0);
}

#[test]
fn must_fail_wheel_ticks_beyond_the_message_cap_are_refused() {
    // One notch = one message; 1024 messages is the cap.
    let over_cap = 1025;
    assert!(matches!(
        WheelPlan::for_notches(over_cap),
        Err(super::ActionError::TargetInvalid { .. })
    ));
    assert!(matches!(
        WheelPlan::for_notches(-over_cap),
        Err(super::ActionError::TargetInvalid { .. })
    ));
}
