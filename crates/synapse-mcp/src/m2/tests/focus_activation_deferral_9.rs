//! #9: when the real activation step is refused by Windows
//! (`SetForegroundWindow` false / lock screen -> `ForegroundActivationRefused`),
//! the focus step must return a typed, retryable `FOREGROUND_ACTIVATION_REFUSED`
//! deferral that names the preserved target — never a generic internal error —
//! and must not activate anything else.
//!
//! Contract: issue #9 "Required behavior" + fix comment (typed
//! FOREGROUND_ACTIVATION_REFUSED, retryable=true, retry_after_ms=1000,
//! continuation.preserved=true, target HWND); m2/foreground_deferral.rs.
//! Existing tests manufacture the A11yError and call `a11y_error_to_focus_mcp`
//! directly; this drives the activation STEP that produces it.
//!
//! Seam (NEEDS-SEAM, see G4 report): `fn activate_focus_window_once_with(
//! before: &ForegroundContext, matched: &ForegroundContext, activate: impl
//! FnOnce(i64) -> synapse_a11y::A11yResult<()>) -> Result<FocusActivation,
//! ErrorData>`; `activate_focus_window_once` must call it with
//! `|hwnd| synapse_a11y::focus_window_with_intent(hwnd, OperatorRequested{..})`.

use std::cell::RefCell;

use serde_json::Value;
use synapse_core::{ForegroundContext, Rect, error_codes};

use super::{activate_focus_window_once_with, missing_foreground_context};
use crate::m2::foreground_deferral::{
    FOREGROUND_RETRY_AFTER_MS, deferred_result_fields, is_deferred,
};

const TARGET: i64 = 0x00fc_0cc0;
const OTHER: i64 = 0x0013_1304;

fn window(hwnd: i64) -> ForegroundContext {
    ForegroundContext {
        hwnd,
        pid: 4242,
        process_name: "Code.exe".to_owned(),
        process_path: r"C:\Apps\Code.exe".to_owned(),
        window_title: "Finance-Manager - Visual Studio Code".to_owned(),
        window_bounds: Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        },
        ..missing_foreground_context()
    }
}

fn data(error: &rmcp::ErrorData) -> &Value {
    error.data.as_ref().expect("structured error data")
}

#[test]
fn must_fail_refused_activation_is_a_retryable_deferral_naming_the_target() {
    let activated = RefCell::new(Vec::new());
    let error =
        activate_focus_window_once_with(&missing_foreground_context(), &window(TARGET), |hwnd| {
            activated.borrow_mut().push(hwnd);
            Err(synapse_a11y::A11yError::foreground_activation_refused(
                format!("SetForegroundWindow returned false for hwnd 0x{hwnd:x}"),
            ))
        })
        .err()
        .expect("a refused activation is not a success");

    assert_eq!(
        *activated.borrow(),
        vec![TARGET],
        "only the matched target is activated"
    );
    let data = data(&error);
    assert_eq!(
        data["code"],
        error_codes::FOREGROUND_ACTIVATION_REFUSED,
        "{data}"
    );
    assert_eq!(data["retryable"], true, "{data}");
    assert_eq!(
        data["retry_after_ms"],
        u64::from(FOREGROUND_RETRY_AFTER_MS),
        "{data}"
    );
    assert_eq!(data["target_hwnd"], TARGET, "{data}");
    assert!(
        is_deferred(&error),
        "target_act reports status=deferred for it"
    );
    let fields = deferred_result_fields(&error).expect("deferral fields");
    assert_eq!(fields["continuation"]["preserved"], true, "{fields}");
    assert_eq!(fields["continuation"]["target_hwnd"], TARGET, "{fields}");
}

#[test]
fn must_fail_internal_activation_failure_is_not_a_deferral() {
    let error = activate_focus_window_once_with(&window(OTHER), &window(TARGET), |_| {
        Err(synapse_a11y::A11yError::internal("UIA worker crashed"))
    })
    .err()
    .expect("an internal failure is not a success");
    assert_eq!(data(&error)["code"], error_codes::OBSERVE_INTERNAL);
    assert!(
        !is_deferred(&error),
        "internal failures are terminal, not deferred"
    );
}

#[test]
fn must_pass_successful_activation_from_no_foreground_is_one_attempt() {
    let activated = RefCell::new(Vec::new());
    let plan =
        activate_focus_window_once_with(&missing_foreground_context(), &window(TARGET), |hwnd| {
            activated.borrow_mut().push(hwnd);
            Ok(())
        })
        .unwrap_or_else(|error| panic!("activation succeeded: {error:?}"));
    assert_eq!(plan.attempts, 1);
    assert_eq!(*activated.borrow(), vec![TARGET]);
}

#[test]
fn must_pass_target_already_foreground_is_not_reactivated() {
    let activated = RefCell::new(Vec::new());
    let plan = activate_focus_window_once_with(&window(TARGET), &window(TARGET), |hwnd| {
        activated.borrow_mut().push(hwnd);
        Ok(())
    })
    .unwrap_or_else(|error| panic!("no-op plan: {error:?}"));
    assert_eq!(plan.attempts, 0);
    assert!(
        activated.borrow().is_empty(),
        "no activation call for the current foreground"
    );
}
