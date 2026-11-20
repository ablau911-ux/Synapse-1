//! Regression tests for #205: the lease must tell an action's auto-acquired
//! hold apart from an explicit `lease_acquire`, so an action finishing cannot
//! release a lease its own session explicitly took over.

use std::sync::{Mutex, MutexGuard, PoisonError};

use synapse_action::{LeaseOutcome, lease, lease::LeaseError};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let _prior = lease::force_clear("lease_auto_explicit_205_reset");
    guard
}

fn ttl() -> std::time::Duration {
    lease::ttl_from_ms(30_000)
}

#[test]
fn must_pass_explicit_acquire_over_an_auto_lease_converts_it_and_the_action_release_is_a_noop() {
    let _guard = serial();
    assert!(matches!(
        lease::try_acquire_auto("s205", ttl()),
        LeaseOutcome::Acquired(_)
    ));

    let (outcome, converted) = lease::try_acquire_explicit("s205", ttl());

    assert!(matches!(outcome, LeaseOutcome::Renewed(_)), "{outcome:?}");
    assert!(converted, "takeover of an auto lease must be reported");
    assert_eq!(lease::release_auto("s205"), Ok(false));
    let status = lease::status();
    assert!(status.held);
    assert_eq!(status.owner_session_id.as_deref(), Some("s205"));
}

#[test]
fn must_pass_an_unconverted_auto_lease_is_released_by_the_action() {
    let _guard = serial();
    assert!(matches!(
        lease::try_acquire_auto("s205", ttl()),
        LeaseOutcome::Acquired(_)
    ));

    assert_eq!(lease::release_auto("s205"), Ok(true));
    assert!(!lease::status().held);
}

#[test]
fn must_pass_a_concurrent_action_renewal_keeps_the_lease_auto() {
    let _guard = serial();
    assert!(matches!(
        lease::try_acquire_auto("s205", ttl()),
        LeaseOutcome::Acquired(_)
    ));
    assert!(matches!(
        lease::try_acquire_auto("s205", ttl()),
        LeaseOutcome::Renewed(_)
    ));

    assert_eq!(lease::release_auto("s205"), Ok(true));
}

#[test]
fn must_fail_an_explicit_lease_stays_explicit_when_an_action_renews_it() {
    let _guard = serial();
    let (outcome, converted) = lease::try_acquire_explicit("s205", ttl());
    assert!(matches!(outcome, LeaseOutcome::Acquired(_)));
    assert!(!converted);
    assert!(matches!(
        lease::try_acquire_auto("s205", ttl()),
        LeaseOutcome::Renewed(_)
    ));

    assert_eq!(lease::release_auto("s205"), Ok(false));
    assert!(lease::status().held);
}

#[test]
fn must_pass_repeated_explicit_acquire_is_a_plain_renewal() {
    let _guard = serial();
    let (_first, first_converted) = lease::try_acquire_explicit("s205", ttl());
    let (second, second_converted) = lease::try_acquire_explicit("s205", ttl());

    assert!(!first_converted && !second_converted);
    assert!(matches!(second, LeaseOutcome::Renewed(_)));
}

#[test]
fn must_pass_a_handed_off_lease_is_explicit() {
    let _guard = serial();
    assert!(matches!(
        lease::try_acquire_auto("s205-from", ttl()),
        LeaseOutcome::Acquired(_)
    ));
    assert!(lease::handoff("s205-from", "s205-to", ttl()).is_ok());

    assert_eq!(lease::release_auto("s205-to"), Ok(false));
    assert!(lease::status().held);
}

#[test]
fn must_fail_release_auto_by_a_non_holder_is_not_held() {
    let _guard = serial();

    // Nobody holds the lease: since #2325 (8cee51df) the release is an
    // idempotent no-op (already released), not a refusal.
    let before = lease::status();
    assert_eq!(lease::release_auto("nobody-205"), Ok(false));
    assert_eq!(lease::status(), before, "a no-op release must not mutate");

    // Another session holds an auto lease: the refusal names it and the lease
    // stays with its owner.
    assert!(matches!(
        lease::try_acquire_auto("s205", ttl()),
        LeaseOutcome::Acquired(_)
    ));
    let error = lease::release_auto("nobody-205");
    assert_eq!(
        error,
        Err(LeaseError::NotHeld {
            session_id: "nobody-205".to_owned(),
            holder: Some("s205".to_owned()),
        })
    );
    if let Err(error) = &error {
        assert_eq!(
            error.code(),
            synapse_core::error_codes::ACTION_FOREGROUND_LEASE_NOT_HELD
        );
    }
    let after = lease::status();
    assert!(after.held);
    assert_eq!(after.owner_session_id.as_deref(), Some("s205"));
    assert_eq!(after.ttl_ms, Some(30_000));
    // The owner's auto lease is still releasable by the owner.
    assert_eq!(lease::release_auto("s205"), Ok(true));
}

#[test]
fn must_pass_restore_auto_if_owner_makes_an_explicit_lease_auto_again() {
    let _guard = serial();
    let (_outcome, _converted) = lease::try_acquire_explicit("s205", ttl());

    assert!(!lease::restore_auto_if_owner("someone-else-205"));
    assert!(lease::restore_auto_if_owner("s205"));
    assert_eq!(lease::release_auto("s205"), Ok(true));
}

#[test]
fn must_fail_an_action_auto_renew_never_shortens_the_sessions_explicit_lease() {
    let _guard = serial();
    let (outcome, _converted) = lease::try_acquire_explicit("s205", lease::ttl_from_ms(30_000));
    assert!(matches!(outcome, LeaseOutcome::Acquired(_)), "{outcome:?}");

    // A foreground action auto-renews with the short default TTL.
    let renewed = lease::try_acquire_auto("s205", lease::ttl_from_ms(5_000));
    assert!(matches!(renewed, LeaseOutcome::Renewed(_)), "{renewed:?}");

    let expires_in_ms = lease::status().expires_in_ms.unwrap_or(0);
    assert!(
        expires_in_ms > 20_000,
        "explicit 30 s lease was shortened to {expires_in_ms} ms by the action"
    );
}

#[test]
fn must_pass_an_action_auto_renew_still_extends_an_explicit_lease_near_expiry() {
    let _guard = serial();
    let (outcome, _converted) = lease::try_acquire_explicit("s205", lease::ttl_from_ms(200));
    assert!(matches!(outcome, LeaseOutcome::Acquired(_)), "{outcome:?}");

    let renewed = lease::try_acquire_auto("s205", lease::ttl_from_ms(5_000));
    assert!(matches!(renewed, LeaseOutcome::Renewed(_)), "{renewed:?}");

    let expires_in_ms = lease::status().expires_in_ms.unwrap_or(0);
    assert!(
        expires_in_ms > 1_000,
        "lease must cover the action: {expires_in_ms} ms"
    );
}
