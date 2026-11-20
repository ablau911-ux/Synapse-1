//! Regression test for #199: the diagnostic rate-limit override resets itself
//! after its TTL, but a reset scheduled by an earlier override must not end a
//! newer override early.

use std::time::Duration;

use synapse_action::{ActionEmitter, ResolvedBackend};

use super::super::diagnostic_rate_limit_reset::schedule_reset;

fn software_capacity(control: &synapse_action::BackendRateLimitControl) -> u32 {
    control.try_snapshot().expect("snapshot").software.capacity
}

#[tokio::test]
async fn must_fail_earlier_override_reset_does_not_end_a_newer_override() {
    let (_handle, _snapshot, emitter) = ActionEmitter::channel();
    let control = emitter.rate_limit_control();
    let default_capacity = software_capacity(&control);
    assert!(default_capacity > 0);

    let (_, first) = control
        .override_backend(ResolvedBackend::Software, 0, 0)
        .expect("first override");
    schedule_reset(control.clone(), ResolvedBackend::Software, 60, first);

    tokio::time::sleep(Duration::from_millis(20)).await;
    let (_, second) = control
        .override_backend(ResolvedBackend::Software, 0, 0)
        .expect("second override");
    schedule_reset(control.clone(), ResolvedBackend::Software, 400, second);

    // The first TTL (60 ms) has expired; the second override must still hold.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        software_capacity(&control),
        0,
        "stale reset ended the newer override early"
    );

    // The second TTL expires and restores the default limiter.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(software_capacity(&control), default_capacity);
}

#[tokio::test]
async fn must_pass_single_override_still_resets_after_its_ttl() {
    let (_handle, _snapshot, emitter) = ActionEmitter::channel();
    let control = emitter.rate_limit_control();
    let default_capacity = software_capacity(&control);

    let (_, generation) = control
        .override_backend(ResolvedBackend::Software, 0, 0)
        .expect("override");
    schedule_reset(control.clone(), ResolvedBackend::Software, 30, generation);
    assert_eq!(software_capacity(&control), 0);

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(software_capacity(&control), default_capacity);
}

#[tokio::test]
async fn must_fail_override_on_another_emitter_does_not_cancel_this_reset() {
    let (_handle_a, _snapshot_a, emitter_a) = ActionEmitter::channel();
    let (_handle_b, _snapshot_b, emitter_b) = ActionEmitter::channel();
    let control_a = emitter_a.rate_limit_control();
    let control_b = emitter_b.rate_limit_control();
    let default_capacity = software_capacity(&control_a);

    let (_, generation_a) = control_a
        .override_backend(ResolvedBackend::Software, 0, 0)
        .expect("override a");
    schedule_reset(
        control_a.clone(),
        ResolvedBackend::Software,
        30,
        generation_a,
    );
    // A later override of a different emitter's limiter is not newer for emitter A.
    let (_, generation_b) = control_b
        .override_backend(ResolvedBackend::Software, 0, 0)
        .expect("override b");
    schedule_reset(
        control_b.clone(),
        ResolvedBackend::Software,
        30,
        generation_b,
    );

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(software_capacity(&control_a), default_capacity);
    assert_eq!(software_capacity(&control_b), default_capacity);
}
