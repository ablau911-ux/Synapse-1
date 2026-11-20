//! #206: `target_claim_status` must tolerate a target whose window is gone, so
//! the agent can still see (and then release) its claim on the dead window.
use synapse_core::error_codes;

use super::{SessionTarget, status_target_liveness};

/// A non-zero HWND that is verified dead at runtime. A hard-coded handle could
/// belong to a live window on a real desktop and fail these tests falsely, so
/// scan upwards from a base value for one the OS reports as not a window
/// (`foreground_context` validates with `IsWindow`).
fn dead_hwnd() -> i64 {
    (0..4096_i64)
        .map(|step| 0xd17ee + step * 2)
        .find(|candidate| synapse_a11y::foreground_context(*candidate).is_err())
        .unwrap_or_else(|| panic!("no dead HWND found among 4096 candidates"))
}

#[test]
fn must_pass_status_of_dead_window_target_reports_not_live_instead_of_erroring() {
    let target = SessionTarget::Window { hwnd: dead_hwnd() };

    let live = status_target_liveness(&target)
        .expect("a non-zero but dead HWND must not fail target_claim_status");

    assert!(!live, "dead window must be reported as not live");
}

#[test]
fn must_pass_status_of_dead_cdp_target_reports_not_live_instead_of_erroring() {
    let target = SessionTarget::Cdp {
        window_hwnd: dead_hwnd(),
        cdp_target_id: "tab-gone".to_owned(),
    };

    let live = status_target_liveness(&target)
        .expect("a CDP target on a dead window must not fail target_claim_status");

    assert!(!live);
}

#[test]
fn must_fail_status_still_rejects_structurally_invalid_targets() {
    let error = status_target_liveness(&SessionTarget::Window { hwnd: 0 })
        .expect_err("zero HWND has no valid claim key shape");

    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(|code| code.as_str()),
        Some(error_codes::TOOL_PARAMS_INVALID)
    );
}
