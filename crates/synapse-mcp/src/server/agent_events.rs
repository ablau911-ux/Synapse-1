//! `CF_AGENT_EVENTS` journal writer (#897).
//!
//! One row per agent lifecycle/telemetry event, keyed `(ts_ns, seq)` through
//! [`synapse_storage::agent_events`]. Writers: HTTP session store (session
//! initialized/restored/deleted), session lifecycle teardown (exited),
//! `act_spawn_agent` (spawn requested/ready/failed), the agent mailbox
//! (message sent/received), the input-lease tools (acquired/released), and
//! the push-telemetry ingress (#899, [`super::agent_event_ingress`]) through
//! which spawned agents self-report turn/tool-call/attention events.
//!
//! # Durability contract (#897 acceptance)
//!
//! [`record_agent_event`] rides the storage batcher (`Db::put_batch`,
//! flushed every 100 ms / 64 KiB): a daemon crash loses at most the
//! in-flight batch. [`record_agent_event_durable`] additionally calls
//! `Db::flush()` so terminal lifecycle facts (exited, spawn failure,
//! session deleted) survive an immediate crash.
//!
//! # Failure contract
//!
//! A journal write failure is never swallowed: it logs a structured
//! `AGENT_EVENT_WRITE_FAILED` error with the full event context and is
//! returned to the caller. Tool handlers journal *after* the primary state
//! mutation commits (except inbox drains, which journal *before* deleting
//! rows so a failure can never lose messages); their errors carry
//! `operation_committed` so callers know whether the primary effect stands.

use std::{
    collections::BTreeSet,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use rmcp::model::ErrorCode;
use serde_json::json;
use synapse_core::{AgentEventKind, AgentEventRecord};
use synapse_storage::{
    CrossCfBatchMutation, Db, StorageError, StorageResult, agent_events::agent_event_key, cf,
    encode_json,
};

use super::ErrorData;
use super::session_registry::{SharedSessionRegistry, unix_time_ms_now};

/// Hard cap on one encoded journal row. Agent events are bounded metadata;
/// anything larger indicates a writer leaking content into the journal.
pub(crate) const MAX_AGENT_EVENT_VALUE_BYTES: usize = 16 * 1024;

/// Process-wide tie-breaker for same-nanosecond events. Ordering authority
/// within one clock tick; wraps harmlessly because `ts_ns` dominates the key.
static NEXT_AGENT_EVENT_SEQ: AtomicU32 = AtomicU32::new(0);

static SESSION_REGISTRY_ACTIVITY_SINK: OnceLock<Mutex<Option<SharedSessionRegistry>>> =
    OnceLock::new();

/// Physical readback of one persisted journal row.
#[derive(Clone, Debug)]
pub(crate) struct AgentEventWriteReadback {
    pub ts_ns: u64,
    pub seq: u32,
    pub value_len_bytes: usize,
}

/// Current unix time in nanoseconds. A clock before the epoch yields 0,
/// which [`AgentEventRecord::validate`] refuses — the failure surfaces
/// instead of journaling rows the TTL filter could never expire correctly.
pub(crate) fn unix_time_ns_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
        .unwrap_or_default()
}

pub(crate) fn install_session_registry_activity_sink(registry: SharedSessionRegistry) {
    let slot = SESSION_REGISTRY_ACTIVITY_SINK.get_or_init(|| Mutex::new(None));
    match slot.lock() {
        Ok(mut guard) => {
            *guard = Some(registry);
            tracing::info!(
                code = "AGENT_EVENT_SESSION_REGISTRY_ACTIVITY_SINK_INSTALLED",
                "agent activity rows will refresh SessionRegistry last_seen"
            );
        }
        Err(_poisoned) => {
            tracing::error!(
                code = "AGENT_EVENT_SESSION_LAST_SEEN_REFRESH_FAILED",
                "could not install session-registry activity sink because the sink lock is poisoned"
            );
        }
    }
}

/// Validates, encodes, and enqueues one event row (batched write path).
///
/// # Errors
///
/// Returns [`StorageError::WriteFailed`] when the record fails validation,
/// exceeds [`MAX_AGENT_EVENT_VALUE_BYTES`], or the storage batcher rejects
/// the write. Every failure is also logged with `AGENT_EVENT_WRITE_FAILED`.
pub(crate) fn record_agent_event(
    db: &Db,
    record: &AgentEventRecord,
) -> StorageResult<AgentEventWriteReadback> {
    let mut readbacks = record_agent_events(db, std::slice::from_ref(record))?;
    readbacks.pop().ok_or_else(|| StorageError::WriteFailed {
        cf_name: cf::CF_AGENT_EVENTS.to_owned(),
        detail: "AGENT_EVENT_WRITE_FAILED: single-record write returned no readback".to_owned(),
    })
}

/// Validates, encodes, and enqueues a batch of event rows in one storage
/// batch. All-or-nothing: any invalid record refuses the whole batch before
/// anything is written.
///
/// This is also the projection choke point for the #898 agent state machine:
/// after the rows commit, they feed [`super::agent_state`], so every journal
/// writer drives lifecycle states and none can bypass them.
///
/// # Errors
///
/// Returns [`StorageError::WriteFailed`] under the same conditions as
/// [`record_agent_event`].
pub(crate) fn record_agent_events(
    db: &Db,
    records: &[AgentEventRecord],
) -> StorageResult<Vec<AgentEventWriteReadback>> {
    let readbacks = record_agent_events_unobserved(db, records)?;
    super::agent_state::observe_recorded_events(db, records);
    refresh_installed_session_registry_activity(records);
    Ok(readbacks)
}

/// Commits agent journal rows together with `CF_SESSIONS` ownership-row
/// mutations in one RocksDB batch.
///
/// Input-lease lifecycle changes use this narrow path so a crash cannot leave
/// a durable ownership marker without its corresponding journal edge (or the
/// inverse). The journal records remain subject to the normal validation and
/// state projection after the atomic write succeeds.
pub(crate) fn record_agent_events_with_session_mutations(
    db: &Db,
    session_deletes: &[Vec<u8>],
    session_puts: &[(Vec<u8>, Vec<u8>)],
    records: &[AgentEventRecord],
) -> StorageResult<Vec<AgentEventWriteReadback>> {
    let mut mutations = Vec::with_capacity(
        session_deletes
            .len()
            .saturating_add(session_puts.len())
            .saturating_add(records.len()),
    );
    mutations.extend(
        session_deletes
            .iter()
            .cloned()
            .map(|key| CrossCfBatchMutation::Delete {
                cf_name: cf::CF_SESSIONS,
                key,
            }),
    );
    mutations.extend(
        session_puts
            .iter()
            .cloned()
            .map(|(key, value)| CrossCfBatchMutation::Put {
                cf_name: cf::CF_SESSIONS,
                key,
                value,
            }),
    );

    let mut readbacks = Vec::with_capacity(records.len());
    for record in records {
        let encoded = validate_and_encode(record).inspect_err(|error| {
            tracing::error!(
                code = "AGENT_EVENT_WRITE_FAILED",
                kind = ?record.kind,
                session_id = ?record.session_id,
                spawn_id = ?record.spawn_id,
                reason_code = ?record.reason_code,
                detail = %error,
                "agent event refused before cross-column-family transition write"
            );
        })?;
        let seq = NEXT_AGENT_EVENT_SEQ.fetch_add(1, Ordering::Relaxed);
        readbacks.push(AgentEventWriteReadback {
            ts_ns: record.ts_ns,
            seq,
            value_len_bytes: encoded.len(),
        });
        mutations.push(CrossCfBatchMutation::Put {
            cf_name: cf::CF_AGENT_EVENTS,
            key: agent_event_key(record.ts_ns, seq),
            value: encoded,
        });
    }

    #[cfg(test)]
    refuse_lease_release_batch_for_test(db, records)?;
    db.mutate_cross_cf_batch_pressure_bypass(mutations)
        .inspect_err(|error| {
            tracing::error!(
                code = "AGENT_EVENT_WRITE_FAILED",
                record_count = records.len(),
                session_delete_count = session_deletes.len(),
                session_put_count = session_puts.len(),
                detail = %error,
                "cross-column-family lease lifecycle transition write failed"
            );
        })?;
    super::agent_state::observe_recorded_events(db, records);
    refresh_installed_session_registry_activity(records);
    Ok(readbacks)
}

#[cfg(test)]
type LeaseReleaseFaults =
    std::collections::BTreeMap<(usize, String), std::sync::Arc<std::sync::atomic::AtomicBool>>;

#[cfg(test)]
fn lease_release_faults() -> &'static std::sync::Mutex<LeaseReleaseFaults> {
    static FAULTS: std::sync::OnceLock<std::sync::Mutex<LeaseReleaseFaults>> =
        std::sync::OnceLock::new();
    FAULTS.get_or_init(Default::default)
}

#[cfg(test)]
pub(crate) struct LeaseReleaseBatchFaultGuard {
    key: (usize, String),
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
impl Drop for LeaseReleaseBatchFaultGuard {
    fn drop(&mut self) {
        self.armed.store(false, Ordering::SeqCst);
        let mut faults = lease_release_faults()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if faults
            .get(&self.key)
            .is_some_and(|value| std::sync::Arc::ptr_eq(value, &self.armed))
        {
            faults.remove(&self.key);
        }
    }
}

#[cfg(test)]
pub(crate) fn fail_next_lease_release_batch_for_test(
    db: &Db,
    session_id: &str,
) -> LeaseReleaseBatchFaultGuard {
    let key = (std::ptr::from_ref(db) as usize, session_id.to_owned());
    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    lease_release_faults()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key.clone(), std::sync::Arc::clone(&armed));
    LeaseReleaseBatchFaultGuard { key, armed }
}

#[cfg(test)]
fn refuse_lease_release_batch_for_test(db: &Db, records: &[AgentEventRecord]) -> StorageResult<()> {
    let faults = lease_release_faults()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for record in records
        .iter()
        .filter(|record| record.kind == synapse_core::AgentEventKind::LeaseReleased)
    {
        let Some(session_id) = record.session_id.as_ref() else {
            continue;
        };
        if faults
            .get(&(std::ptr::from_ref(db) as usize, session_id.clone()))
            .is_some_and(|armed| armed.swap(false, Ordering::SeqCst))
        {
            return Err(synapse_storage::StorageError::WriteFailed {
                cf_name: cf::CF_AGENT_EVENTS.to_owned(),
                detail: "injected lease release atomic batch failure".to_owned(),
            });
        }
    }
    Ok(())
}

fn refresh_installed_session_registry_activity(records: &[AgentEventRecord]) {
    let Some(registry) = installed_session_registry_activity_sink() else {
        return;
    };
    let refreshed =
        refresh_session_registry_activity_from_agent_events(&registry, records, unix_time_ms_now());
    if !refreshed.is_empty() {
        tracing::debug!(
            code = "AGENT_EVENT_SESSION_LAST_SEEN_REFRESHED",
            refreshed_session_count = refreshed.len(),
            session_ids = ?refreshed,
            "readback=SessionRegistry edge=agent_activity_heartbeat"
        );
    }
}

fn installed_session_registry_activity_sink() -> Option<SharedSessionRegistry> {
    let slot = SESSION_REGISTRY_ACTIVITY_SINK.get()?;
    match slot.lock() {
        Ok(guard) => guard.clone(),
        Err(_poisoned) => {
            tracing::error!(
                code = "AGENT_EVENT_SESSION_LAST_SEEN_REFRESH_FAILED",
                "could not read session-registry activity sink because the sink lock is poisoned"
            );
            None
        }
    }
}

pub(crate) fn refresh_session_registry_activity_from_agent_events(
    registry: &SharedSessionRegistry,
    records: &[AgentEventRecord],
    now_unix_ms: u64,
) -> Vec<String> {
    let mut guard = match registry.lock() {
        Ok(guard) => guard,
        Err(_poisoned) => {
            tracing::error!(
                code = "AGENT_EVENT_SESSION_LAST_SEEN_REFRESH_FAILED",
                "could not lock session registry while refreshing activity heartbeat"
            );
            return Vec::new();
        }
    };
    let mut refreshed = BTreeSet::new();
    let mut activity_record_count = 0usize;
    for record in records {
        if !agent_event_counts_as_session_activity(record.kind) {
            continue;
        }
        activity_record_count += 1;
        refreshed.extend(guard.record_agent_activity(
            record.session_id.as_deref(),
            record.spawn_id.as_deref(),
            now_unix_ms,
        ));
    }
    let refreshed: Vec<String> = refreshed.into_iter().collect();
    if activity_record_count > 0 {
        tracing::debug!(
            code = "AGENT_EVENT_SESSION_LAST_SEEN_REFRESH_READBACK",
            activity_record_count,
            refreshed_session_count = refreshed.len(),
            session_ids = ?refreshed,
            "readback=SessionRegistry edge=agent_activity_heartbeat"
        );
    }
    refreshed
}

fn agent_event_counts_as_session_activity(kind: AgentEventKind) -> bool {
    matches!(
        kind,
        AgentEventKind::ToolCallStarted
            | AgentEventKind::ToolCallFinished
            | AgentEventKind::TurnStarted
            | AgentEventKind::TurnFinished
            | AgentEventKind::MessageSent
            | AgentEventKind::MessageReceived
    )
}

/// The raw journal write path, without the state-machine projection. Only
/// the state machine itself uses this directly (its own transition rows must
/// not re-enter the reducer).
pub(crate) fn record_agent_events_unobserved(
    db: &Db,
    records: &[AgentEventRecord],
) -> StorageResult<Vec<AgentEventWriteReadback>> {
    let mut rows = Vec::with_capacity(records.len());
    let mut readbacks = Vec::with_capacity(records.len());
    for record in records {
        let encoded = validate_and_encode(record).inspect_err(|error| {
            tracing::error!(
                code = "AGENT_EVENT_WRITE_FAILED",
                kind = ?record.kind,
                session_id = ?record.session_id,
                spawn_id = ?record.spawn_id,
                reason_code = ?record.reason_code,
                detail = %error,
                "agent event refused before write"
            );
        })?;
        let seq = NEXT_AGENT_EVENT_SEQ.fetch_add(1, Ordering::Relaxed);
        readbacks.push(AgentEventWriteReadback {
            ts_ns: record.ts_ns,
            seq,
            value_len_bytes: encoded.len(),
        });
        rows.push((agent_event_key(record.ts_ns, seq), encoded));
    }
    if rows.is_empty() {
        return Ok(readbacks);
    }
    db.put_batch(cf::CF_AGENT_EVENTS, rows)
        .inspect_err(|error| {
            tracing::error!(
                code = "AGENT_EVENT_WRITE_FAILED",
                record_count = records.len(),
                first_kind = ?records.first().map(|record| record.kind),
                detail = %error,
                "agent event batch enqueue failed"
            );
        })?;
    for (record, readback) in records.iter().zip(&readbacks) {
        tracing::debug!(
            code = "AGENT_EVENT_RECORDED",
            kind = ?record.kind,
            ts_ns = readback.ts_ns,
            seq = readback.seq,
            session_id = ?record.session_id,
            spawn_id = ?record.spawn_id,
            value_len_bytes = readback.value_len_bytes,
            "readback=CF_AGENT_EVENTS edge=enqueued"
        );
    }
    Ok(readbacks)
}

/// [`record_agent_event`] plus an explicit `Db::flush()` so the row is
/// readable and crash-durable before this returns. Reserved for terminal
/// lifecycle events (exited, killed, spawn failure, session deleted).
///
/// # Errors
///
/// Returns [`StorageError::WriteFailed`] from the write or the flush.
pub(crate) fn record_agent_event_durable(
    db: &Db,
    record: &AgentEventRecord,
) -> StorageResult<AgentEventWriteReadback> {
    let readback = record_agent_event(db, record)?;
    db.flush().inspect_err(|error| {
        tracing::error!(
            code = "AGENT_EVENT_WRITE_FAILED",
            kind = ?record.kind,
            ts_ns = readback.ts_ns,
            seq = readback.seq,
            detail = %error,
            "agent event terminal flush failed"
        );
    })?;
    Ok(readback)
}

fn validate_and_encode(record: &AgentEventRecord) -> StorageResult<Vec<u8>> {
    record
        .validate()
        .map_err(|detail| StorageError::WriteFailed {
            cf_name: cf::CF_AGENT_EVENTS.to_owned(),
            detail,
        })?;
    let encoded = encode_json(record)?;
    if encoded.len() > MAX_AGENT_EVENT_VALUE_BYTES {
        return Err(StorageError::WriteFailed {
            cf_name: cf::CF_AGENT_EVENTS.to_owned(),
            detail: format!(
                "AGENT_EVENT_INVALID: encoded row is {} bytes, cap is {MAX_AGENT_EVENT_VALUE_BYTES}; journal rows are bounded metadata, never content",
                encoded.len()
            ),
        });
    }
    Ok(encoded)
}

/// Maps a registry `agent_kind` onto the OTel `gen_ai.provider.name`
/// well-known values. Unknown kinds stay unattributed rather than guessed.
pub(crate) fn provider_for_agent_kind(agent_kind: &str) -> Option<String> {
    match agent_kind {
        "claude" => Some("anthropic".to_owned()),
        "codex" => Some("openai".to_owned()),
        "local-model" => Some("local".to_owned()),
        _ => None,
    }
}

/// Maps a journal write failure into a tool error that states whether the
/// primary operation already committed before the journal refused.
pub(crate) fn agent_event_tool_error(
    tool: &'static str,
    error: &StorageError,
    operation_committed: bool,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} could not journal its agent event to CF_AGENT_EVENTS: {error}{}",
            if operation_committed {
                " (the underlying operation already committed; storage needs attention before its effects are auditable)"
            } else {
                ""
            }
        ),
        Some(json!({
            "code": error.code(),
            "reason": "agent_event_journal_write_failed",
            "tool": tool,
            "operation_committed": operation_committed,
        })),
    )
}
