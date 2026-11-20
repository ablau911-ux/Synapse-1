//! Regression test for #95: a lease row left by a prior process is closed by
//! startup reconciliation with exactly one `LeaseReleased` event, and a
//! further restart does not journal a duplicate.

use tempfile::TempDir;

use super::session_lease_orphan_sweep_tests::{lease_row, put_row, row_exists};
use super::*;
use crate::test_support;

fn seed_prior_process_row(dir: &std::path::Path, session_id: &str) -> anyhow::Result<()> {
    let db = Db::open(&dir.join("db"), synapse_core::SCHEMA_VERSION)?;
    put_row(&db, &lease_row(session_id, 0, 0))?;
    Ok(())
}

fn reasons_after_rebuild(
    dir: &std::path::Path,
    session_id: &str,
) -> anyhow::Result<(Vec<String>, bool)> {
    let service = test_support::service_in(dir)?;
    let db =
        session_continuity_db_from_state(&service.m3_state_handle()).map_err(anyhow::Error::msg)?;
    Ok((
        test_support::lease_released_reasons(&db, session_id)?,
        row_exists(&db, session_id)?,
    ))
}

#[test]
fn must_fail_startup_reconciliation_never_closes_a_current_process_row() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("restart_reconcile_95");
    let temp = TempDir::new()?;
    // A current-process row is live only while this process physically holds the lease;
    // a stamped row with no physical lease is an orphan and must be closed (#93/#95).
    let outcome = lease::try_acquire("current-process-session", lease::ttl_from_ms(60_000));
    assert!(matches!(outcome, lease::LeaseOutcome::Acquired(_)));
    let generation = current_lease_process_generation();
    {
        let db = Db::open(&temp.path().join("db"), synapse_core::SCHEMA_VERSION)?;
        put_row(
            &db,
            &lease_row(
                "current-process-session",
                generation.process_id,
                generation.started_at_unix_ms,
            ),
        )?;
    }

    let (reasons, row_present) = reasons_after_rebuild(temp.path(), "current-process-session")?;
    assert!(
        reasons.is_empty(),
        "live-process row must not be released: {reasons:?}"
    );
    assert!(row_present);
    Ok(())
}

#[test]
fn must_pass_startup_reconciliation_closes_prior_process_row_exactly_once() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("restart_reconcile_95");
    let temp = TempDir::new()?;
    seed_prior_process_row(temp.path(), "prior-process-session")?;

    let (reasons, row_present) = reasons_after_rebuild(temp.path(), "prior-process-session")?;
    assert_eq!(reasons, vec!["daemon_restart_reconciliation".to_owned()]);
    assert!(!row_present);

    // Another restart: row is gone, no duplicate release event.
    let (reasons, row_present) = reasons_after_rebuild(temp.path(), "prior-process-session")?;
    assert_eq!(reasons, vec!["daemon_restart_reconciliation".to_owned()]);
    assert!(!row_present);
    Ok(())
}
