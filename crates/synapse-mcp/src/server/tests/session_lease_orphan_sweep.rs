//! Regression tests for #93/#94/#95: persisted lease rows whose lease is gone
//! are closed with exactly one `LeaseReleased` event, idempotently; a row that
//! matches the live physical owner is never touched.

use std::num::NonZeroUsize;

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig, server::SynapseService,
    test_support,
};

pub(super) fn service(dir: &std::path::Path) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(dir.join("db")),
            Some(dir.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).ok_or_else(|| anyhow::anyhow!("nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

pub(super) fn lease_row(
    session_id: &str,
    process_id: u32,
    started_at: u64,
) -> PersistedSessionLease {
    PersistedSessionLease {
        schema_version: 1,
        session_id: session_id.to_owned(),
        stored_at_unix_ms: 1_000,
        renewed_at_unix_ms: 1_000,
        ttl_ms: 5_000,
        expires_at_unix_ms: 6_000,
        owner_process_id: process_id,
        owner_process_started_at_unix_ms: started_at,
    }
}

pub(super) fn put_row(db: &Db, row: &PersistedSessionLease) -> anyhow::Result<()> {
    db.put_batch_pressure_bypass(
        cf::CF_SESSIONS,
        [(
            session_lease_key(&row.session_id),
            synapse_storage::encode_json(row)?,
        )],
    )?;
    Ok(())
}

fn event_rows(db: &Db) -> anyhow::Result<usize> {
    Ok(db.scan_cf(cf::CF_AGENT_EVENTS)?.len())
}

pub(super) fn row_exists(db: &Db, session_id: &str) -> anyhow::Result<bool> {
    Ok(cf_row_exists(db, &session_lease_key(session_id))?)
}

#[test]
fn must_fail_orphaned_row_is_closed_once_with_one_release_event() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("orphan_sweep_test");
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    let db =
        session_continuity_db_from_state(&service.m3_state_handle()).map_err(anyhow::Error::msg)?;
    put_row(&db, &lease_row("ghost-session", 0, 0))?;
    let events_before = event_rows(&db)?;

    let state = service.m3_state_handle();
    assert_eq!(
        reconcile_orphaned_session_lease_rows(&state, "orphan_sweep")
            .map_err(anyhow::Error::msg)?,
        1
    );
    assert!(!row_exists(&db, "ghost-session")?);
    assert_eq!(event_rows(&db)?, events_before + 1);

    // Idempotent: no row left, no second event.
    assert_eq!(
        reconcile_orphaned_session_lease_rows(&state, "orphan_sweep")
            .map_err(anyhow::Error::msg)?,
        0
    );
    assert_eq!(event_rows(&db)?, events_before + 1);
    Ok(())
}

#[test]
fn must_pass_row_matching_the_live_physical_owner_is_preserved() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("orphan_sweep_test");
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    let state = service.m3_state_handle();
    let db = session_continuity_db_from_state(&state).map_err(anyhow::Error::msg)?;
    let outcome = lease::try_acquire("live-owner", lease::ttl_from_ms(5_000));
    assert!(matches!(outcome, lease::LeaseOutcome::Acquired(_)));
    let generation = current_lease_process_generation();
    put_row(
        &db,
        &lease_row(
            "live-owner",
            generation.process_id,
            generation.started_at_unix_ms,
        ),
    )?;
    let events_before = event_rows(&db)?;

    assert_eq!(
        reconcile_orphaned_session_lease_rows(&state, "orphan_sweep")
            .map_err(anyhow::Error::msg)?,
        0
    );
    assert!(row_exists(&db, "live-owner")?);
    assert_eq!(event_rows(&db)?, events_before);
    Ok(())
}

#[test]
fn must_fail_undecodable_row_is_skipped_and_other_orphans_are_still_closed() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("orphan_sweep_test");
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    let state = service.m3_state_handle();
    let db = session_continuity_db_from_state(&state).map_err(anyhow::Error::msg)?;
    db.put_batch_pressure_bypass(
        cf::CF_SESSIONS,
        [(session_lease_key("garbled"), b"{not-json".to_vec())],
    )?;
    put_row(&db, &lease_row("ghost-two", 0, 0))?;

    assert_eq!(
        reconcile_orphaned_session_lease_rows(&state, "orphan_sweep")
            .map_err(anyhow::Error::msg)?,
        1
    );
    assert!(row_exists(&db, "garbled")?);
    assert!(!row_exists(&db, "ghost-two")?);
    Ok(())
}
