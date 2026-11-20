//! Regression test for #94: releasing a lease that has no persisted lifecycle
//! row (auto-acquired foreground lease, or a row already closed by the sweep
//! or the hotkey path) is a clean success, not STORAGE_CORRUPTED.

use tempfile::TempDir;

use super::*;
use crate::test_support;

#[test]
fn must_pass_rowless_release_after_dashboard_force_release_is_success() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("rowless_release_94");
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let db = service
        .m3_state_handle()
        .lock()
        .map_err(|_error| anyhow::anyhow!("m3 state poisoned"))?
        .ensure_storage()?;
    // Auto-acquired foreground lease: physical only, never persisted.
    assert!(matches!(
        lease::try_acquire("auto-fg-session", lease::ttl_from_ms(5_000)),
        LeaseOutcome::Acquired(_)
    ));
    assert!(lease::force_clear_if_owner("auto-fg-session", "dashboard_force_release").is_some());
    let events_before = db.scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)?.len();

    reconcile_lease_release_after_mutation(&service, "auto-fg-session", "dashboard_force_release")
        .map_err(|error| anyhow::anyhow!("rowless release must succeed: {error:?}"))?;

    // No persisted acquisition => nothing to balance, no event synthesized.
    assert_eq!(
        db.scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)?.len(),
        events_before
    );
    Ok(())
}

#[test]
fn must_fail_force_clear_by_a_non_owner_leaves_the_owner_lease_held() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("rowless_release_94");
    assert!(matches!(
        lease::try_acquire("real-owner", lease::ttl_from_ms(5_000)),
        LeaseOutcome::Acquired(_)
    ));
    assert!(
        lease::force_clear_if_owner("not-the-owner", "dashboard_force_release").is_none(),
        "a non-owner must not clear someone else's lease"
    );
    // Direct readback of the lease source of truth: still held by the owner.
    let status = lease::status();
    assert!(status.held, "the owner's lease must stay held: {status:?}");
    assert_eq!(status.owner_session_id.as_deref(), Some("real-owner"));
    assert!(lease::force_clear_if_owner("real-owner", "test_cleanup").is_some());
    assert!(!lease::status().held, "test cleanup releases the lease");
    Ok(())
}

#[test]
fn must_pass_second_release_after_a_prior_closer_is_idempotent_success() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("rowless_release_94");
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let db = service
        .m3_state_handle()
        .lock()
        .map_err(|_error| anyhow::anyhow!("m3 state poisoned"))?
        .ensure_storage()?;
    let events_before = db.scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)?.len();
    let sessions_before = db.scan_cf(synapse_storage::cf::CF_SESSIONS)?.len();
    for _ in 0..2 {
        reconcile_lease_release_after_mutation(&service, "already-closed", "explicit_release")
            .map_err(|error| anyhow::anyhow!("idempotent release must succeed: {error:?}"))?;
        // A rowless release synthesizes nothing: no event, no session row.
        assert_eq!(
            db.scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)?.len(),
            events_before
        );
        assert_eq!(
            db.scan_cf(synapse_storage::cf::CF_SESSIONS)?.len(),
            sessions_before
        );
    }
    Ok(())
}
