//! Regression test for #95: a naturally expired lease is journaled
//! `lease_expired` by the expiry cleanup, which must run `lease::status()`
//! itself so the 250 ms orphan sweep never wins the race with reason
//! `orphan_sweep` on a quiet daemon.

use std::time::Duration;

use tempfile::TempDir;

use crate::test_support;

#[tokio::test]
async fn must_fail_a_live_lease_is_never_journaled_as_expired() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("expired_journal_95");
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let lifecycle = service
        .session_lifecycle_state()
        .map_err(|error| anyhow::anyhow!("lifecycle state: {error:?}"))?;
    let db = service
        .m3_state_handle()
        .lock()
        .map_err(|_error| anyhow::anyhow!("m3 state poisoned"))?
        .ensure_storage()?;
    assert!(matches!(
        synapse_action::lease::try_acquire(
            "live-session",
            synapse_action::lease::ttl_from_ms(synapse_action::MAX_LEASE_TTL_MS),
        ),
        synapse_action::LeaseOutcome::Acquired(_)
    ));
    service
        .persist_session_lease("live-session", &synapse_action::lease::status())
        .map_err(|error| anyhow::anyhow!("persist lease row: {error:?}"))?;

    lifecycle.cleanup_expired_lease_inputs_once().await;
    lifecycle.reconcile_orphaned_lease_rows_once().await;

    assert!(
        test_support::lease_released_reasons(&db, "live-session")?.is_empty(),
        "a lease inside its TTL must not be released"
    );
    assert!(synapse_action::lease::status().held);
    assert!(synapse_action::lease::force_clear_if_owner("live-session", "test_cleanup").is_some());
    Ok(())
}

#[tokio::test]
async fn must_pass_natural_expiry_is_journaled_lease_expired_not_orphan_sweep() -> anyhow::Result<()>
{
    let _serial = test_support::lease_serial("expired_journal_95");
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let lifecycle = service
        .session_lifecycle_state()
        .map_err(|error| anyhow::anyhow!("lifecycle state: {error:?}"))?;
    let db = service
        .m3_state_handle()
        .lock()
        .map_err(|_error| anyhow::anyhow!("m3 state poisoned"))?
        .ensure_storage()?;
    assert!(matches!(
        synapse_action::lease::try_acquire(
            "expiring-session",
            synapse_action::lease::ttl_from_ms(synapse_action::MIN_LEASE_TTL_MS),
        ),
        synapse_action::LeaseOutcome::Acquired(_)
    ));
    service
        .persist_session_lease("expiring-session", &synapse_action::lease::status())
        .map_err(|error| anyhow::anyhow!("persist lease row: {error:?}"))?;

    // Quiet daemon: nothing reads the lease while it lapses.
    tokio::time::sleep(Duration::from_millis(
        synapse_action::MIN_LEASE_TTL_MS + 150,
    ))
    .await;
    lifecycle.cleanup_expired_lease_inputs_once().await;
    lifecycle.reconcile_orphaned_lease_rows_once().await;

    assert_eq!(
        test_support::lease_released_reasons(&db, "expiring-session")?,
        vec!["lease_expired".to_owned()]
    );
    Ok(())
}
