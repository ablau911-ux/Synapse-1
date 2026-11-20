//! Regression tests for #166: the mailbox row readback digest (also copied
//! into the `message_sent` journal row) must be a keyed HMAC, not a plain
//! SHA-256 of the whole message row, so low-entropy payloads cannot be
//! brute-forced offline from the journal.

use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use super::mailbox_sweep_throttle_tests::{register, send, service};
use super::*;

const NOW: u64 = 10_000;

fn plain_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

fn keyed(bytes: &[u8]) -> String {
    format!(
        "hmac-sha256:{}",
        crate::content_digest::keyed_digest_hex(bytes)
    )
}

fn seeded() -> anyhow::Result<(TempDir, SynapseService)> {
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for session in ["sender", "r1"] {
        register(&service, session, 1_000)?;
    }
    Ok((temp, service))
}

fn peek(service: &SynapseService) -> anyhow::Result<AgentInboxResponse> {
    Ok(service.agent_inbox_impl_at(
        AgentInboxParams {
            drain: false,
            max_messages: 10,
            kinds: Vec::new(),
        },
        "r1",
        NOW + 1,
    )?)
}

#[test]
fn must_fail_send_readback_digest_is_keyed_not_plain_sha256() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let db = service.mailbox_db()?;
    let response = send(&service, "r1", 600_000, NOW)?;
    let readback = &response.storage_readback;
    let stored = db
        .get_cf_exact(cf::CF_KV, readback.row_key.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("mailbox row missing"))?;

    assert!(
        readback.value_sha256.starts_with("hmac-sha256:"),
        "unmarked or unkeyed digest: {}",
        readback.value_sha256
    );
    assert_eq!(readback.value_sha256, keyed(&stored));
    assert_ne!(readback.value_sha256, plain_sha256(&stored));
    Ok(())
}

#[test]
fn must_fail_message_sent_journal_digest_is_keyed_hmac() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let db = service.mailbox_db()?;
    let response = send(&service, "r1", 600_000, NOW)?;
    let stored = db
        .get_cf_exact(cf::CF_KV, response.storage_readback.row_key.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("mailbox row missing"))?;

    let (rows, _more) = db.scan_cf_from(cf::CF_AGENT_EVENTS, &[], 1_000_000)?;
    let sent: Vec<synapse_core::AgentEventRecord> = rows
        .iter()
        .filter_map(|(_key, value)| synapse_storage::decode_json(value).ok())
        .filter(|record: &synapse_core::AgentEventRecord| {
            record.kind == synapse_core::AgentEventKind::MessageSent
        })
        .collect();
    assert_eq!(sent.len(), 1, "exactly one message_sent journal row");
    let digest = sent[0]
        .payload
        .get("value_sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("journal row has no value_sha256"))?;

    assert!(
        digest.starts_with("hmac-sha256:"),
        "unmarked or unkeyed journal digest: {digest}"
    );
    assert_eq!(digest, keyed(&stored));
    assert_ne!(digest, plain_sha256(&stored));
    assert_eq!(digest, response.storage_readback.value_sha256);
    Ok(())
}

#[test]
fn must_pass_inbox_readback_digest_is_keyed_and_equal_to_send_readback() -> anyhow::Result<()> {
    let _mailbox_serial = crate::test_support::MAILBOX_SWEEP_SERIAL.blocking_lock();
    let (_temp, service) = seeded()?;
    let sent = send(&service, "r1", 600_000, NOW)?;

    let first = peek(&service)?;
    let second = peek(&service)?;
    let row = first
        .readback_rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("inbox readback row missing"))?;

    assert!(row.value_sha256.starts_with("hmac-sha256:"));
    assert_eq!(
        row.value_sha256, sent.storage_readback.value_sha256,
        "same row bytes must keep producing the same digest"
    );
    assert_eq!(
        second.readback_rows.first().map(|r| r.value_sha256.clone()),
        Some(row.value_sha256.clone())
    );
    Ok(())
}
