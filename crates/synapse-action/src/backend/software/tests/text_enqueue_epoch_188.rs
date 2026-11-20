//! #188 follow-up: the software text backend measures the release interrupt
//! against the epoch read when the action was ENQUEUED. An interrupt raised
//! after enqueue but before the run starts (the foreground monitor firing while
//! the chunk still waits in the queue) must stop the chunk before its first key.

use super::run_type_steps;
use crate::{
    ActionError,
    backend::text_dispatch::{TextDispatchInput, TextDispatchStep},
    test_support_epoch::RELEASE_EPOCH_TEST_LOCK,
};

// The release epoch is process-global; every epoch test in this crate takes the
// one crate-wide lock.

fn steps(text: &str) -> Vec<TextDispatchStep> {
    text.encode_utf16()
        .map(|unit| TextDispatchStep {
            iki_ms_before: 1,
            inputs: vec![TextDispatchInput::UnicodeUnit(unit)],
        })
        .collect()
}

fn units(sent: &[TextDispatchInput]) -> Vec<u16> {
    sent.iter()
        .filter_map(|input| match input {
            TextDispatchInput::UnicodeUnit(unit) => Some(*unit),
            TextDispatchInput::VirtualKey(_) => None,
        })
        .collect()
}

#[test]
fn must_fail_interrupt_after_enqueue_before_start_sends_no_key() {
    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    // The chunk is enqueued: its epoch is read now.
    let enqueued_epoch = crate::operator_release_epoch();
    // The foreground monitor raises the interrupt while the chunk is queued.
    crate::request_release_interrupt();
    // Reading the epoch at chunk start (the pre-fix behavior) would see the
    // already-bumped value and type everything; the enqueue epoch must win.
    assert_ne!(enqueued_epoch, crate::operator_release_epoch());
    // Only now does the backend start the chunk.
    let mut sent = Vec::new();
    let result = run_type_steps(
        steps("secret"),
        enqueued_epoch,
        |_ms| false,
        |input| {
            sent.push(input);
            Ok(())
        },
    );
    let error = result.expect_err("an interrupt raised after enqueue must stop the chunk");
    assert!(
        matches!(error, ActionError::SafetyOperatorHotkeyFired { .. }),
        "unexpected error: {error:?}"
    );
    assert!(sent.is_empty(), "no key of the chunk may be sent: {sent:?}");
}

#[test]
fn must_pass_no_interrupt_types_full_text_exactly_once() {
    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    let text = "hello 188";
    let enqueued_epoch = crate::operator_release_epoch();
    let mut sent = Vec::new();
    run_type_steps(
        steps(text),
        enqueued_epoch,
        |_ms| false,
        |input| {
            sent.push(input);
            Ok(())
        },
    )
    .expect("no interrupt, the whole text is typed");
    assert_eq!(units(&sent), text.encode_utf16().collect::<Vec<_>>());
}

// #281 r3: these call the production entry point, including its readiness
// decision. Only the native probe, clock/sleep, and input sink are injected.
// The focus-event counter stays at zero, as with an unstarted stdio hook.
// No test edits the dispatch plan or waits before calling type_text.
#[test]
fn must_fail_already_ready_probed_on_every_type_call_without_sleep_281_r3() {
    use super::{readiness_test_support::Fixture, type_text};
    use std::time::Duration;
    use synapse_core::KeystrokeDynamics;

    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    let fixture = Fixture::install(Some(Duration::ZERO));
    for (index, text) in ["a", "abc", "aa", "שלום", "🙂x"].into_iter().enumerate() {
        let before = fixture.snapshot();
        let result = type_text(text, &KeystrokeDynamics::Burst, None);
        let after = fixture.snapshot();
        assert!(result.is_ok(), "ready field must type: {result:?}");
        assert_eq!(
            after.probes.len(),
            index + 1,
            "every type_text call must synchronously probe even without a focus event"
        );
        assert!(after.sleeps.is_empty(), "ready input must not sleep");
        assert_eq!(after.elapsed, Duration::ZERO);
        assert_eq!(
            units(&after.sent[before.sent.len()..]),
            text.encode_utf16().collect::<Vec<_>>(),
            "exact text must be emitted once"
        );
        assert_eq!(
            units(&after.received[before.received.len()..]),
            text.encode_utf16().collect::<Vec<_>>(),
            "ready field must receive the entire prefix"
        );
    }
}

#[test]
fn must_fail_not_ready_then_ready_waits_before_first_key_281_r3() {
    use super::{readiness_test_support::Fixture, type_text};
    use std::time::Duration;
    use synapse_core::KeystrokeDynamics;

    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    for text in ["a", "abc", "aa", "שלום", "🙂x"] {
        let ready_at = Duration::from_millis(3);
        let fixture = Fixture::install(Some(ready_at));
        let result = type_text(text, &KeystrokeDynamics::Burst, None);
        let after = fixture.snapshot();
        assert!(
            result.is_ok(),
            "eventual readiness must permit typing: {result:?}"
        );
        assert!(
            after.probes.len() >= 2,
            "production must retry its failed readiness probe: {:?}",
            after.probes
        );
        assert!(
            !after.sleeps.is_empty(),
            "production must wait while unready"
        );
        assert!(after.elapsed >= ready_at && after.elapsed <= Duration::from_millis(250));
        assert!(
            after.sent_at.first().is_some_and(|at| *at >= ready_at),
            "no first key may precede readiness: {:?}",
            after.sent_at
        );
        assert_eq!(units(&after.sent), text.encode_utf16().collect::<Vec<_>>());
        assert_eq!(
            units(&after.received),
            text.encode_utf16().collect::<Vec<_>>(),
            "field readback must retain every UTF-16 unit, including the first"
        );
    }
}

#[test]
fn must_fail_never_ready_errors_at_250ms_with_zero_keys_281_r3() {
    use super::{readiness_test_support::Fixture, type_text};
    use std::time::Duration;
    use synapse_core::KeystrokeDynamics;

    let _guard = RELEASE_EPOCH_TEST_LOCK.blocking_lock();
    let fixture = Fixture::install(None);
    let result = type_text("secret", &KeystrokeDynamics::Burst, None);
    let after = fixture.snapshot();
    assert!(
        matches!(&result, Err(ActionError::InputTargetNotReady { detail }) if detail.contains("250 ms")),
        "unready target must return a typed readiness timeout: {result:?}"
    );
    assert_eq!(
        after.elapsed,
        Duration::from_millis(250),
        "total wait budget"
    );
    assert!(
        after.probes.len() >= 2,
        "must poll readiness until the deadline"
    );
    assert!(
        after.probes.iter().all(|(at, timeout_ms)| {
            *timeout_ms > 0
                && Duration::from_millis(u64::from(*timeout_ms))
                    <= Duration::from_millis(250).saturating_sub(*at)
        }),
        "native probe must receive only the remaining budget: {:?}",
        after.probes
    );
    assert!(
        after.sent.is_empty(),
        "timeout must send zero keys: {:?}",
        after.sent
    );
    assert!(
        after.received.is_empty(),
        "timeout must leave the field untouched"
    );
}
