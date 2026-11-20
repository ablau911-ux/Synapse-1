//! #188: foreground typing must honor the operator/release interrupt instead of
//! sending the rest of the text until it finishes.

use super::run_type_steps;
use crate::{
    ActionError,
    backend::text_dispatch::{TextDispatchInput, TextDispatchStep},
    test_support_epoch::RELEASE_EPOCH_TEST_LOCK,
};

// The release epoch is process-global; every epoch test in this crate takes the
// one crate-wide lock so another file's interrupt cannot land inside this run.

fn steps(count: u16) -> Vec<TextDispatchStep> {
    (0..count)
        .map(|unit| TextDispatchStep {
            iki_ms_before: 1,
            inputs: vec![TextDispatchInput::UnicodeUnit(u16::from(b'a') + unit)],
        })
        .collect()
}

#[test]
fn must_fail_release_interrupt_mid_run_stops_remaining_keys() {
    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    let mut sent = 0_usize;
    let result = run_type_steps(
        steps(6),
        crate::operator_release_epoch(),
        |_ms| false,
        |_input| {
            sent += 1;
            if sent == 2 {
                crate::request_release_interrupt();
            }
            Ok(())
        },
    );
    let error = result.expect_err("typing must stop after the release interrupt");
    let ActionError::SafetyOperatorHotkeyFired { detail } = &error else {
        panic!("unexpected error: {error:?}");
    };
    assert!(
        detail.contains("after 2 input unit(s)"),
        "the refusal must name the units already sent: {detail}"
    );
    assert_eq!(sent, 2, "no key may be sent after the interrupt");
}

#[test]
fn must_fail_interrupt_during_inter_key_delay_stops_before_next_key() {
    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    let mut sent = 0_usize;
    let result = run_type_steps(
        steps(3),
        crate::operator_release_epoch(),
        |_ms| true,
        |_input| {
            sent += 1;
            Ok(())
        },
    );
    let error = result.expect_err("an interrupted delay must abort typing");
    assert!(matches!(
        error,
        ActionError::SafetyOperatorHotkeyFired { .. }
    ));
    assert_eq!(sent, 0);
}

#[test]
fn must_pass_uninterrupted_run_sends_every_key() {
    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    let mut sent = 0_usize;
    run_type_steps(
        steps(4),
        crate::operator_release_epoch(),
        |_ms| false,
        |_input| {
            sent += 1;
            Ok(())
        },
    )
    .expect("no interrupt, typing completes");
    assert_eq!(sent, 4);
}
