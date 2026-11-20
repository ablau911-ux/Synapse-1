//! Regression tests for #31: receipt drains and the send cap check must be
//! serialized against other mailbox mutations by the process-wide mailbox lock.
//! The test holds the lock, starts the operation on another thread, and asserts
//! it cannot complete until the lock is released (deterministic, no timing race
//! on the storage layer itself).

use std::{
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::mailbox_sweep_throttle_tests::{register, send, service};
use super::*;

const NOW: u64 = 50_000;
const BLOCK_PROBE: Duration = Duration::from_millis(400);

fn assert_blocked_while_locked<T: Send>(operation: impl FnOnce() -> T + Send) -> anyhow::Result<T> {
    thread::scope(|scope| {
        let guard = MAILBOX_DRAINS
            .lock()
            .map_err(|_error| anyhow::anyhow!("mailbox lock poisoned"))?;
        let handle = scope.spawn(operation);
        let started = Instant::now();
        while started.elapsed() < BLOCK_PROBE {
            anyhow::ensure!(
                !handle.is_finished(),
                "operation completed while the mailbox lock was held by another thread"
            );
            thread::sleep(Duration::from_millis(20));
        }
        drop(guard);
        handle
            .join()
            .map_err(|_panic| anyhow::anyhow!("operation thread panicked"))
    })
}

fn seeded() -> anyhow::Result<(TempDir, SynapseService)> {
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for session in ["sender", "recipient"] {
        register(&service, session, 1_000)?;
    }
    Ok((temp, service))
}

#[test]
fn must_pass_receipts_drain_waits_for_the_mailbox_lock() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let result = assert_blocked_while_locked(|| {
        service.agent_receipts_impl_at(
            AgentReceiptsParams {
                drain: true,
                max_receipts: 10,
            },
            "sender",
            NOW,
        )
    })?;
    assert_eq!(result?.mode, "drain");
    Ok(())
}

#[test]
fn must_pass_send_waits_for_the_mailbox_lock_before_the_capped_insert() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let result = assert_blocked_while_locked(|| send(&service, "recipient", 60_000, NOW))?;
    assert_eq!(result?.to_session, "recipient");
    let db = service.mailbox_db()?;
    assert_eq!(
        queue_depth_for_recipient(&db, "recipient", NOW + 1)?,
        1,
        "the message lands once the lock is released"
    );
    Ok(())
}

fn error_code(error: &ErrorData) -> String {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn must_fail_send_to_a_full_mailbox_is_refused_and_writes_nothing() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    send(&service, "recipient", 3_600_000, NOW)?;
    let db = service.mailbox_db()?;
    let prefix = mailbox_recipient_prefix("recipient");
    let template = db
        .scan_cf_prefix(cf::CF_KV, prefix.as_bytes())?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no seed row"))?;
    // Fill the recipient inbox exactly to the cap with live clones.
    let mut batch = Vec::new();
    for index in 1..MAX_INBOX_ROWS_PER_RECIPIENT {
        let mut key = template.0.clone();
        key.extend_from_slice(format!("/cap-{index:08}").as_bytes());
        batch.push((key, template.1.clone()));
    }
    db.put_batch_pressure_bypass(cf::CF_KV, batch)?;
    assert_eq!(
        queue_depth_for_recipient(&db, "recipient", NOW + 1)?,
        MAX_INBOX_ROWS_PER_RECIPIENT
    );

    let error = service
        .agent_send_impl_at(
            AgentSendParams {
                to_session: "recipient".to_owned(),
                kind: "handoff".to_owned(),
                payload: json!({"over": "cap"}),
                artifact_handle: None,
                ttl_ms: 60_000,
                request_receipt: false,
            },
            "sender",
            NOW + 1,
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("a send past the cap must be refused"))?;
    assert_eq!(
        error_code(&error),
        error_codes::ACTION_QUEUE_FULL,
        "{error:?}"
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, prefix.as_bytes())?.len(),
        MAX_INBOX_ROWS_PER_RECIPIENT,
        "a refused send must not insert a row"
    );
    Ok(())
}

#[test]
fn must_fail_invalid_receipts_request_is_refused_without_waiting_for_the_lock() -> anyhow::Result<()>
{
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let outcome = thread::scope(|scope| {
        let guard = MAILBOX_DRAINS
            .lock()
            .map_err(|_error| anyhow::anyhow!("mailbox lock poisoned"))?;
        let handle = scope.spawn(|| {
            service.agent_receipts_impl_at(
                AgentReceiptsParams {
                    drain: true,
                    max_receipts: 0,
                },
                "sender",
                NOW,
            )
        });
        let started = Instant::now();
        while !handle.is_finished() && started.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(10));
        }
        let finished_while_locked = handle.is_finished();
        drop(guard);
        let result = handle
            .join()
            .map_err(|_panic| anyhow::anyhow!("receipts thread panicked"))?;
        anyhow::Ok((finished_while_locked, result))
    })?;
    let (finished_while_locked, result) = outcome;
    assert!(
        finished_while_locked,
        "parameter validation must refuse before taking the mailbox lock"
    );
    let error = result
        .err()
        .ok_or_else(|| anyhow::anyhow!("max_receipts=0 must be refused"))?;
    assert_eq!(
        error_code(&error),
        error_codes::TOOL_PARAMS_INVALID,
        "{error:?}"
    );
    Ok(())
}
