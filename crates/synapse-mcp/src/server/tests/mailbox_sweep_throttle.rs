//! Regression tests for #57: the fleet-wide expiry sweep is throttled instead
//! of decoding every mailbox row on every send.

use std::num::NonZeroUsize;

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

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

pub(super) fn register(service: &SynapseService, session_id: &str, now: u64) -> anyhow::Result<()> {
    let mut registry = service
        .session_registry_ref()
        .lock()
        .map_err(|_error| anyhow::anyhow!("session registry lock poisoned"))?;
    registry.record_seen(session_id, Some("test".to_owned()), now);
    Ok(())
}

pub(super) fn send(
    service: &SynapseService,
    to: &str,
    ttl_ms: u64,
    now: u64,
) -> anyhow::Result<AgentSendResponse> {
    Ok(service.agent_send_impl_at(
        AgentSendParams {
            to_session: to.to_owned(),
            kind: "handoff".to_owned(),
            payload: json!({"t": now}),
            artifact_handle: None,
            ttl_ms,
            request_receipt: false,
        },
        "sender",
        now,
    )?)
}

fn rows_for(db: &Db, to_session: &str) -> anyhow::Result<usize> {
    let mut count = 0;
    for (_key, value) in db.scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())? {
        let message: AgentMailboxMessage = synapse_storage::decode_json(&value)?;
        if message.to_session == to_session {
            count += 1;
        }
    }
    Ok(count)
}

pub(super) fn message_rows(db: &Db) -> anyhow::Result<usize> {
    Ok(db
        .scan_cf_prefix(cf::CF_KV, MESSAGE_PREFIX.as_bytes())?
        .len())
}

#[test]
fn must_pass_busy_sweep_is_retried_on_the_next_call_inside_the_interval() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for session in ["sender", "r1", "r2"] {
        register(&service, session, 1_000)?;
    }
    let db = service.mailbox_db()?;
    send(&service, "r2", 1_000, 10_000)?;
    let due = 10_000 + mailbox_sweep_interval_ms() + 1_001;
    assert!(mailbox_sweep_interval_ms() > 1);
    assert_eq!(rows_for(&db, "r2")?, 1);
    {
        let _busy = MAILBOX_DRAINS
            .lock()
            .map_err(|_error| anyhow::anyhow!("mailbox lock poisoned"))?;
        assert_eq!(
            throttled_cleanup_expired_mailbox_rows(&db, due),
            MailboxSweep::default()
        );
        assert_eq!(rows_for(&db, "r2")?, 1, "busy sweep must not delete");
    }
    let retried = throttled_cleanup_expired_mailbox_rows(&db, due + 1);
    assert_eq!(
        retried.deleted, 1,
        "busy sweep must return the throttle slot for immediate retry"
    );
    assert_eq!(
        rows_for(&db, "r2")?,
        0,
        "retry must physically reclaim the expired row"
    );
    assert_eq!(
        throttled_cleanup_expired_mailbox_rows(&db, due + 2).deleted,
        0
    );
    Ok(())
}

#[test]
fn must_fail_sweep_runs_at_most_once_per_interval_per_database() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for session in ["sender", "r1", "r2"] {
        register(&service, session, 1_000)?;
    }
    let db = service.mailbox_db()?;
    let interval = mailbox_sweep_interval_ms();
    assert!(interval > 0, "default throttle interval must be positive");

    // First send sweeps (nothing to delete yet) and starts the interval.
    send(&service, "r1", 600_000, 10_000)?;
    // A short-lived message to another recipient expires 1s later.
    send(&service, "r2", 1_000, 10_100)?;
    assert_eq!(message_rows(&db)?, 2);

    // Inside the interval the expired foreign row is NOT swept by a send.
    send(&service, "r1", 600_000, 10_100 + interval / 2)?;
    assert_eq!(
        rows_for(&db, "r2")?,
        1,
        "expired r2 row must survive a throttled send"
    );

    // After the interval the next send sweeps and reclaims it.
    send(&service, "r1", 600_000, 10_000 + interval + 1)?;
    assert_eq!(
        rows_for(&db, "r2")?,
        0,
        "the sweep after the interval reclaims it"
    );
    Ok(())
}

#[test]
fn must_fail_independent_databases_do_not_share_throttle_state() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (temp_a, temp_b) = (TempDir::new()?, TempDir::new()?);
    let (a, b) = (service(temp_a.path())?, service(temp_b.path())?);
    for svc in [&a, &b] {
        for session in ["sender", "r1", "r2"] {
            register(svc, session, 1_000)?;
        }
        send(svc, "r1", 600_000, 10_000)?;
        send(svc, "r2", 1_000, 10_100)?;
    }
    let interval = mailbox_sweep_interval_ms();
    let late = 10_000 + interval + 1;
    // Each database owns its own last-sweep time, so both reclaim.
    send(&a, "r1", 600_000, late)?;
    send(&b, "r1", 600_000, late)?;
    assert_eq!(rows_for(&*a.mailbox_db()?, "r2")?, 0);
    assert_eq!(rows_for(&*b.mailbox_db()?, "r2")?, 0);
    Ok(())
}

#[test]
fn must_pass_sweep_reclaims_exactly_the_expired_rows_and_keeps_live_ones() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for session in ["sender", "r1", "r2"] {
        register(&service, session, 1_000)?;
    }
    let db = service.mailbox_db()?;
    send(&service, "r1", 600_000, 10_000)?;
    // Expires at exactly 10_100 + 1_000 = 11_100.
    send(&service, "r2", 1_000, 10_100)?;
    assert_eq!(message_rows(&db)?, 2);

    // One millisecond before expiry the row is still live.
    let early = cleanup_expired_mailbox_rows(&db, 11_099)?;
    assert_eq!(early.deleted, 0);
    assert_eq!(rows_for(&db, "r2")?, 1);

    // At the expiry instant it is reclaimed; the live r1 row is kept.
    let sweep = cleanup_expired_mailbox_rows(&db, 11_100)?;
    assert_eq!(sweep.deleted, 1);
    assert_eq!(sweep.corrupt_skipped, 0);
    assert_eq!(rows_for(&db, "r2")?, 0);
    assert_eq!(rows_for(&db, "r1")?, 1);
    Ok(())
}

#[test]
fn must_pass_sweeping_send_after_the_interval_keeps_live_rows_and_delivers() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for session in ["sender", "r1"] {
        register(&service, session, 1_000)?;
    }
    let db = service.mailbox_db()?;
    send(&service, "r1", 3_600_000, 10_000)?;
    let late = 10_000 + mailbox_sweep_interval_ms() + 1;
    let response = send(&service, "r1", 3_600_000, late)?;
    assert_eq!(response.to_session, "r1");
    assert_eq!(response.queue_depth_after, 2);
    assert_eq!(rows_for(&db, "r1")?, 2, "a sweep never deletes live rows");
    Ok(())
}
