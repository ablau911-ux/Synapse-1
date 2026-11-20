//! Regression test for #94: the 250 ms orphan sweep must not close the
//! hotkey-displaced owner's lease row between K1 (operator preempt) and K2
//! (`operator_hotkey_preempted` reconciliation). The sweep defers while the
//! operator holds the lease, so K2 journals the fixed reason.

use tempfile::TempDir;

use super::session_lease_orphan_sweep_tests::{lease_row, put_row, row_exists};
use super::*;
use crate::test_support;

#[test]
fn must_pass_k2_journals_exactly_one_operator_hotkey_release() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("hotkey_sweep_94");
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let state = service.m3_state_handle();
    let db = session_continuity_db_from_state(&state).map_err(anyhow::Error::msg)?;
    let generation = current_lease_process_generation();
    put_row(
        &db,
        &lease_row(
            "preempted-owner",
            generation.process_id,
            generation.started_at_unix_ms,
        ),
    )?;
    let readback = reconcile_persisted_session_lease_row(
        &state,
        "preempted-owner",
        "operator_hotkey_preempted",
    )
    .map_err(anyhow::Error::msg)?;
    assert!(readback.release_event_written);
    assert!(!row_exists(&db, "preempted-owner")?);
    assert_eq!(
        test_support::lease_released_reasons(&db, "preempted-owner")?,
        vec!["operator_hotkey_preempted".to_owned()]
    );
    Ok(())
}

#[test]
fn must_fail_sweep_defers_to_k2_while_operator_holds_the_preempted_lease() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("hotkey_sweep_94");
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let state = service.m3_state_handle();
    let db = session_continuity_db_from_state(&state).map_err(anyhow::Error::msg)?;
    let generation = current_lease_process_generation();
    assert!(matches!(
        lease::try_acquire("displaced-owner", lease::ttl_from_ms(5_000)),
        lease::LeaseOutcome::Acquired(_)
    ));
    put_row(
        &db,
        &lease_row(
            "displaced-owner",
            generation.process_id,
            generation.started_at_unix_ms,
        ),
    )?;
    put_row(&db, &lease_row("unrelated-ghost", 0, 0))?;

    // K1: operator preempts the displaced owner.
    let prior = lease::force_preempt("hotkey_sweep_94");
    assert_eq!(
        prior.and_then(|status| status.owner_session_id).as_deref(),
        Some("displaced-owner")
    );

    // A sweep tick lands between K1 and K2: it must not touch any row.
    assert_eq!(
        reconcile_orphaned_session_lease_rows(&state, "orphan_sweep")
            .map_err(anyhow::Error::msg)?,
        0
    );
    assert!(row_exists(&db, "displaced-owner")?);
    assert!(row_exists(&db, "unrelated-ghost")?);

    // K2 closes the displaced owner's row with the fixed reason.
    let readback = reconcile_persisted_session_lease_row(
        &state,
        "displaced-owner",
        "operator_hotkey_preempted",
    )
    .map_err(anyhow::Error::msg)?;
    assert!(readback.release_event_written);
    assert_eq!(
        test_support::lease_released_reasons(&db, "displaced-owner")?,
        vec!["operator_hotkey_preempted".to_owned()]
    );

    // K2 clears the operator lease; the sweep resumes for unrelated orphans.
    assert!(
        lease::force_clear_if_owner(synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID, "k2_done")
            .is_some()
    );
    assert_eq!(
        reconcile_orphaned_session_lease_rows(&state, "orphan_sweep")
            .map_err(anyhow::Error::msg)?,
        1
    );
    assert!(!row_exists(&db, "unrelated-ghost")?);
    assert_eq!(
        test_support::lease_released_reasons(&db, "displaced-owner")?.len(),
        1
    );
    Ok(())
}
