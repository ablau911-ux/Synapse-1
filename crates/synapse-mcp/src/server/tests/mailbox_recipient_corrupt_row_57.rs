//! Regression tests for #57: corrupt or wrong-schema rows inside the recipient's
//! own inbox (`scan_inbox`) or sender's own receipt box (`scan_receipts`) must be
//! skipped without failing send, broadcast, inbox drain, or receipts drain.

use std::thread;

use tempfile::TempDir;

use super::mailbox_sweep_throttle_tests::{register, send, service};
use super::*;

const NOW: u64 = 20_000;

fn seeded() -> anyhow::Result<(TempDir, SynapseService)> {
    let temp = TempDir::new()?;
    let svc = service(temp.path())?;
    for session in ["sender", "r1", "r2"] {
        register(&svc, session, 1_000)?;
    }
    Ok((temp, svc))
}

fn plant_corrupt_inbox_rows(db: &Db, session_id: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let prefix = mailbox_recipient_prefix(session_id);
    let bad_json_key = format!("{prefix}corrupt-json").into_bytes();
    let bad_schema_key = format!("{prefix}corrupt-schema").into_bytes();
    let wrong_schema = serde_json::json!({
        "schema_version": SCHEMA_VERSION + 99,
        "message_id": "agentmsg-bad",
        "row_key": String::from_utf8_lossy(&bad_schema_key),
        "from_session": "sender",
        "to_session": session_id,
        "kind": "handoff",
        "payload": {},
        "artifact_handle": null,
        "sent_at_unix_ms": NOW,
        "ttl_ms": 60_000,
        "expires_at_unix_ms": NOW + 60_000,
        "delivery_attempts": 0,
        "request_receipt": false
    });
    db.put_batch(
        cf::CF_KV,
        [
            (bad_json_key.clone(), b"{not-valid-json".to_vec()),
            (
                bad_schema_key.clone(),
                synapse_storage::encode_json(&wrong_schema)?,
            ),
        ],
    )?;
    Ok((bad_json_key, bad_schema_key))
}

fn plant_corrupt_receipt_rows(db: &Db, owner_session: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let prefix = receipt_recipient_prefix(owner_session);
    let bad_json_key = format!("{prefix}corrupt-rcpt-json").into_bytes();
    let bad_schema_key = format!("{prefix}corrupt-rcpt-schema").into_bytes();
    let wrong_schema = serde_json::json!({
        "schema_version": RECEIPT_SCHEMA_VERSION + 99,
        "receipt_id": "receipt-bad",
        "row_key": String::from_utf8_lossy(&bad_schema_key),
        "from_session": owner_session,
        "recipient_session": "r1",
        "message_id": "agentmsg-bad",
        "message_kind": "handoff",
        "status": "read",
        "read_at_unix_ms": NOW,
        "expires_at_unix_ms": NOW + RECEIPT_TTL_MS
    });
    db.put_batch(
        cf::CF_KV,
        [
            (bad_json_key.clone(), b"{broken-receipt".to_vec()),
            (
                bad_schema_key.clone(),
                synapse_storage::encode_json(&wrong_schema)?,
            ),
        ],
    )?;
    Ok((bad_json_key, bad_schema_key))
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
fn must_pass_send_and_drain_survive_corrupt_rows_in_recipient_inbox_57() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, svc) = seeded()?;
    let db = svc.mailbox_db()?;
    let (bad_json_key, bad_schema_key) = plant_corrupt_inbox_rows(&db, "r1")?;
    let prefix = mailbox_recipient_prefix("r1");
    assert_eq!(db.scan_cf_prefix(cf::CF_KV, prefix.as_bytes())?.len(), 2);

    let sent = svc.agent_send_impl_at(
        AgentSendParams {
            to_session: "r1".to_owned(),
            kind: "handoff".to_owned(),
            payload: json!({"step": 1}),
            artifact_handle: None,
            ttl_ms: 60_000,
            request_receipt: true,
        },
        "sender",
        NOW,
    )?;
    assert_eq!(sent.to_session, "r1");
    assert_eq!(sent.queue_depth_after, 1);

    let drained = svc.agent_inbox_impl_at(
        AgentInboxParams {
            drain: true,
            max_messages: 10,
            kinds: Vec::new(),
        },
        "r1",
        NOW + 10,
    )?;
    assert_eq!(drained.returned_count, 1);
    assert_eq!(drained.deleted_count, 1);
    assert_eq!(drained.queue_depth_after, 0);
    assert_eq!(drained.messages[0].message_id, sent.message_id);
    assert!(db.get_cf_exact(cf::CF_KV, &bad_json_key)?.is_some());
    assert!(db.get_cf_exact(cf::CF_KV, &bad_schema_key)?.is_some());
    Ok(())
}

#[test]
fn must_pass_receipts_drain_survives_corrupt_rows_in_sender_receipt_box_57() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, svc) = seeded()?;
    let db = svc.mailbox_db()?;
    let sent = svc.agent_send_impl_at(
        AgentSendParams {
            to_session: "r1".to_owned(),
            kind: "handoff".to_owned(),
            payload: json!({"ack": true}),
            artifact_handle: None,
            ttl_ms: 60_000,
            request_receipt: true,
        },
        "sender",
        NOW,
    )?;
    svc.agent_inbox_impl_at(
        AgentInboxParams {
            drain: true,
            max_messages: 10,
            kinds: Vec::new(),
        },
        "r1",
        NOW + 5,
    )?;
    let (bad_json_key, bad_schema_key) = plant_corrupt_receipt_rows(&db, "sender")?;
    let prefix = receipt_recipient_prefix("sender");
    assert_eq!(db.scan_cf_prefix(cf::CF_KV, prefix.as_bytes())?.len(), 3);

    let receipts = svc.agent_receipts_impl_at(
        AgentReceiptsParams {
            drain: true,
            max_receipts: 10,
        },
        "sender",
        NOW + 10,
    )?;
    assert_eq!(receipts.returned_count, 1);
    assert_eq!(receipts.deleted_count, 1);
    assert_eq!(receipts.receipts[0].message_id, sent.message_id);
    assert!(db.get_cf_exact(cf::CF_KV, &bad_json_key)?.is_some());
    assert!(db.get_cf_exact(cf::CF_KV, &bad_schema_key)?.is_some());
    Ok(())
}

#[test]
fn must_fail_concurrent_drains_with_corrupt_recipient_row_never_double_deliver_57()
-> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, svc) = seeded()?;
    let db = svc.mailbox_db()?;
    send(&svc, "r1", 100, NOW)?;
    let live = send(&svc, "r1", 60_000, NOW + 1)?;
    let (bad_json_key, _) = plant_corrupt_inbox_rows(&db, "r1")?;

    let (first, second) = thread::scope(|scope| {
        let a = scope.spawn(|| {
            svc.agent_inbox_impl_at(
                AgentInboxParams {
                    drain: true,
                    max_messages: 10,
                    kinds: Vec::new(),
                },
                "r1",
                NOW + 500,
            )
        });
        let b = scope.spawn(|| {
            svc.agent_inbox_impl_at(
                AgentInboxParams {
                    drain: true,
                    max_messages: 10,
                    kinds: Vec::new(),
                },
                "r1",
                NOW + 500,
            )
        });
        (a.join(), b.join())
    });
    let first = first.map_err(|_p| anyhow::anyhow!("drain thread a panicked"))??;
    let second = second.map_err(|_p| anyhow::anyhow!("drain thread b panicked"))??;
    assert_eq!(first.returned_count + second.returned_count, 1);
    assert_eq!(first.deleted_count + second.deleted_count, 1);
    assert_eq!(
        first.expired_rows_deleted + second.expired_rows_deleted,
        1,
        "expired row reclaimed once"
    );
    let delivered_id = first
        .messages
        .first()
        .or_else(|| second.messages.first())
        .ok_or_else(|| anyhow::anyhow!("expected one delivered message"))?
        .message_id
        .clone();
    assert_eq!(delivered_id, live.message_id);
    assert!(db.get_cf_exact(cf::CF_KV, &bad_json_key)?.is_some());
    Ok(())
}

#[test]
fn must_fail_invalid_inbox_params_refuse_without_touching_corrupt_or_live_rows_57()
-> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, svc) = seeded()?;
    let db = svc.mailbox_db()?;
    send(&svc, "r1", 60_000, NOW)?;
    plant_corrupt_inbox_rows(&db, "r1")?;
    let prefix = mailbox_recipient_prefix("r1");
    assert_eq!(db.scan_cf_prefix(cf::CF_KV, prefix.as_bytes())?.len(), 3);

    let err = svc
        .agent_inbox_impl_at(
            AgentInboxParams {
                drain: true,
                max_messages: 0,
                kinds: Vec::new(),
            },
            "r1",
            NOW + 10,
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("max_messages=0 must fail"))?;
    assert_eq!(error_code(&err), error_codes::TOOL_PARAMS_INVALID);
    assert_eq!(db.scan_cf_prefix(cf::CF_KV, prefix.as_bytes())?.len(), 3);
    Ok(())
}
