use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use rmcp::ErrorData;
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_action::{LeaseStatus, lease};
use synapse_core::error_codes;
use synapse_storage::{Db, cf};

use crate::m3::SharedM3State;

use super::{
    CdpTargetOwner, SessionTarget, SynapseService, m1_tools::validate_target_window, mcp_error,
};

const SESSION_TARGET_PREFIX: &str = "mcp/session-target/v1/";
const SESSION_LEASE_PREFIX: &str = "mcp/session-lease/v1/";
const SESSION_CDP_TARGET_OWNER_PREFIX: &str = "mcp/session-cdp-target-owner/v1/";
const LEASE_RECONCILIATION_REASON: &str = "daemon_restart_reconciliation";

#[path = "session_lease_identity.rs"]
mod lease_identity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LeaseProcessGeneration {
    process_id: u32,
    started_at_unix_ms: u64,
}

static LEASE_PROCESS_GENERATION: OnceLock<LeaseProcessGeneration> = OnceLock::new();

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedSessionTarget {
    schema_version: u32,
    session_id: String,
    stored_at_unix_ms: u64,
    target: SessionTarget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedSessionLease {
    schema_version: u32,
    session_id: String,
    stored_at_unix_ms: u64,
    renewed_at_unix_ms: u64,
    ttl_ms: u64,
    expires_at_unix_ms: u64,
    #[serde(default)]
    owner_process_id: u32,
    #[serde(default)]
    owner_process_started_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PersistedCdpTargetOwner {
    pub schema_version: u32,
    pub owner_key: String,
    pub stored_at_unix_ms: u64,
    pub owner_session_id: String,
    pub owner_client_name: Option<String>,
    pub owner_agent_kind: String,
    pub owner_started_at_unix_ms: Option<u64>,
    pub owner: CdpTargetOwner,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct TargetContinuityCleanupReadback {
    pub row_existed_before: bool,
    pub row_exists_after: bool,
    pub row_deleted: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct LeaseReleaseReconciliationReadback {
    pub session_id: String,
    pub row_existed_before: bool,
    pub row_exists_after: bool,
    pub row_deleted: bool,
    pub release_event_written: bool,
    pub reason_code: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct LeaseHandoffContinuityReadback {
    pub from_row_existed_before: bool,
    pub from_row_exists_after: bool,
    pub from_row_deleted: bool,
    pub to_row_exists_after: bool,
    pub to_row_session_id: Option<String>,
}

impl SynapseService {
    pub(super) fn persist_session_target(
        &self,
        session_id: &str,
        target: &SessionTarget,
    ) -> Result<(), ErrorData> {
        let row = PersistedSessionTarget {
            schema_version: 1,
            session_id: session_id.to_owned(),
            stored_at_unix_ms: unix_ms_now(),
            target: target.clone(),
        };
        let encoded = synapse_storage::encode_json(&row).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode persisted session target failed: {error}"),
            )
        })?;
        let db = self.session_continuity_db()?;
        db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(session_target_key(session_id), encoded)])
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        tracing::info!(
            code = "MCP_SESSION_TARGET_PERSISTED",
            session_id,
            "persisted active session target to CF_SESSIONS"
        );
        Ok(())
    }

    pub(super) fn delete_persisted_session_target(
        &self,
        session_id: &str,
    ) -> Result<(), ErrorData> {
        let db = self.session_continuity_db()?;
        let key = session_target_key(session_id);
        let existed_before =
            cf_row_exists(&db, &key).map_err(|error| mcp_error(error.code(), error.to_string()))?;
        delete_exact_session_row(&db, key.clone())?;
        let exists_after =
            cf_row_exists(&db, &key).map_err(|error| mcp_error(error.code(), error.to_string()))?;
        if exists_after {
            return Err(mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!("persisted session target row still exists after delete for {session_id}"),
            ));
        }
        tracing::info!(
            code = "MCP_SESSION_TARGET_DELETED",
            session_id,
            existed_before,
            "deleted active session target from CF_SESSIONS"
        );
        Ok(())
    }

    pub(super) fn delete_persisted_session_target_if_matches(
        &self,
        session_id: &str,
        target: &SessionTarget,
    ) -> Result<bool, ErrorData> {
        let Some(persisted) = self.read_persisted_session_target(session_id)? else {
            return Ok(false);
        };
        if persisted.target != *target {
            tracing::warn!(
                code = "MCP_SESSION_TARGET_DELETE_SKIPPED",
                session_id,
                persisted = ?persisted.target,
                requested = ?target,
                "persisted active session target no longer matches requested cleanup target"
            );
            return Ok(false);
        }
        self.delete_persisted_session_target(session_id)?;
        Ok(true)
    }

    pub(super) fn restore_session_target_if_needed(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionTarget>, ErrorData> {
        if let Some(target) = self.memory_session_target(session_id)? {
            return Ok(Some(target));
        }
        let Some(persisted) = self.read_persisted_session_target(session_id)? else {
            return Ok(None);
        };
        validate_restored_target(session_id, &persisted.target)?;
        {
            let mut guard = self.session_targets_ref().lock().map_err(|_err| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "session target registry lock poisoned while restoring persisted target",
                )
            })?;
            guard.insert(session_id.to_owned(), persisted.target.clone());
        }
        tracing::info!(
            code = "MCP_SESSION_TARGET_RESTORED",
            session_id,
            stored_at_unix_ms = persisted.stored_at_unix_ms,
            "restored active session target from CF_SESSIONS"
        );
        Ok(Some(persisted.target))
    }

    pub(super) fn persist_cdp_target_owner(
        &self,
        owner_key: &str,
        owner: &CdpTargetOwner,
    ) -> Result<(), ErrorData> {
        let owner_read = self.session_registry_read_for_persistence(&owner.session_id)?;
        let row = PersistedCdpTargetOwner {
            schema_version: 1,
            owner_key: owner_key.to_owned(),
            stored_at_unix_ms: unix_ms_now(),
            owner_session_id: owner.session_id.clone(),
            owner_client_name: owner_read
                .as_ref()
                .and_then(|read| read.client_name.clone()),
            owner_agent_kind: owner_read
                .as_ref()
                .map_or_else(|| "unknown".to_owned(), |read| read.agent_kind.clone()),
            owner_started_at_unix_ms: owner_read.as_ref().map(|read| read.started_at_unix_ms),
            owner: owner.clone(),
        };
        let encoded = synapse_storage::encode_json(&row).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode persisted CDP target owner failed: {error}"),
            )
        })?;
        let db = self.session_continuity_db()?;
        db.put_batch_pressure_bypass(
            cf::CF_SESSIONS,
            [(
                cdp_target_owner_row_key(owner_key, &owner.cdp_target_id),
                encoded,
            )],
        )
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        tracing::info!(
            code = "MCP_SESSION_CDP_TARGET_OWNER_PERSISTED",
            owner_session_id = %owner.session_id,
            owner_key = %owner_key,
            hwnd = owner.window_hwnd,
            endpoint = %owner.endpoint,
            cdp_target_id = %owner.cdp_target_id,
            "readback=CF_SESSIONS after=cdp_target_owner_persisted"
        );
        Ok(())
    }

    pub(super) fn delete_persisted_cdp_target_owner(
        &self,
        owner_key: &str,
        cdp_target_id: &str,
    ) -> Result<bool, ErrorData> {
        let db = self.session_continuity_db()?;
        delete_persisted_cdp_target_owner_from_db(&db, owner_key, cdp_target_id)
            .map_err(|error| mcp_error(error_codes::STORAGE_CORRUPTED, error))
    }

    pub(super) fn read_persisted_cdp_target_owners_for_target_id(
        &self,
        cdp_target_id: &str,
    ) -> Result<Vec<(String, PersistedCdpTargetOwner)>, ErrorData> {
        let db = self.session_continuity_db()?;
        let prefix = cdp_target_owner_target_prefix(cdp_target_id);
        let rows = db
            .scan_cf_prefix(cf::CF_SESSIONS, &prefix)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let mut decoded = Vec::new();
        for (row_key, value) in rows {
            let row =
                synapse_storage::decode_json::<PersistedCdpTargetOwner>(&value).map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!(
                            "decode persisted CDP target owner failed for target {cdp_target_id:?}: {error}"
                        ),
                    )
                })?;
            validate_persisted_cdp_target_owner(cdp_target_id, &row)?;
            if row_key != cdp_target_owner_row_key(&row.owner_key, &row.owner.cdp_target_id) {
                return Err(mcp_error(
                    error_codes::STORAGE_CORRUPTED,
                    format!(
                        "persisted CDP target owner row key mismatch for target {cdp_target_id:?}: row_key={} owner_key={}",
                        String::from_utf8_lossy(&row_key),
                        row.owner_key
                    ),
                ));
            }
            decoded.push((row.owner_key.clone(), row));
        }
        decoded.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(decoded)
    }

    pub(super) fn persisted_cdp_target_owner_session_ids(
        &self,
    ) -> Result<BTreeSet<String>, ErrorData> {
        let db = self.session_continuity_db()?;
        read_persisted_cdp_target_owner_session_ids_from_db(&db)
            .map_err(|error| mcp_error(error_codes::STORAGE_CORRUPTED, error))
    }

    pub(super) fn persist_session_lease(
        &self,
        session_id: &str,
        status: &LeaseStatus,
    ) -> Result<(), ErrorData> {
        let ttl_ms = status.ttl_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist unheld input lease: missing ttl_ms",
            )
        })?;
        let expires_in_ms = status.expires_in_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist unheld input lease: missing expires_in_ms",
            )
        })?;
        if status.owner_session_id.as_deref() != Some(session_id) {
            return Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!(
                    "cannot persist input lease for {session_id}: status owner is {:?}",
                    status.owner_session_id
                ),
            ));
        }
        let now = unix_ms_now();
        let generation = current_lease_process_generation();
        let row = PersistedSessionLease {
            schema_version: 1,
            session_id: session_id.to_owned(),
            stored_at_unix_ms: now,
            renewed_at_unix_ms: now.saturating_sub(status.renewed_at_ms_ago.unwrap_or_default()),
            ttl_ms,
            expires_at_unix_ms: now.saturating_add(expires_in_ms),
            owner_process_id: generation.process_id,
            owner_process_started_at_unix_ms: generation.started_at_unix_ms,
        };
        let encoded = lease_identity::encode_lease_row(&row, status).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode persisted session lease failed: {error}"),
            )
        })?;
        let db = self.session_continuity_db()?;
        db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(session_lease_key(session_id), encoded)])
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        tracing::info!(
            code = "MCP_SESSION_LEASE_PERSISTED",
            session_id,
            ttl_ms,
            expires_at_unix_ms = row.expires_at_unix_ms,
            "persisted active input lease intent to CF_SESSIONS"
        );
        Ok(())
    }

    /// Persists a newly acquired lease and its `LeaseAcquired` journal edge in
    /// one RocksDB transition. A process crash can therefore leave neither an
    /// unjournaled open row nor a journal edge without its matching row.
    pub(super) fn persist_session_lease_with_acquisition_event(
        &self,
        session_id: &str,
        status: &LeaseStatus,
    ) -> Result<(), ErrorData> {
        let ttl_ms = status.ttl_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist unheld input lease: missing ttl_ms",
            )
        })?;
        let expires_in_ms = status.expires_in_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist unheld input lease: missing expires_in_ms",
            )
        })?;
        if status.owner_session_id.as_deref() != Some(session_id) {
            return Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!(
                    "cannot persist input lease for {session_id}: status owner is {:?}",
                    status.owner_session_id
                ),
            ));
        }
        let now = unix_ms_now();
        let generation = current_lease_process_generation();
        let row = PersistedSessionLease {
            schema_version: 1,
            session_id: session_id.to_owned(),
            stored_at_unix_ms: now,
            renewed_at_unix_ms: now.saturating_sub(status.renewed_at_ms_ago.unwrap_or_default()),
            ttl_ms,
            expires_at_unix_ms: now.saturating_add(expires_in_ms),
            owner_process_id: generation.process_id,
            owner_process_started_at_unix_ms: generation.started_at_unix_ms,
        };
        let encoded = lease_identity::encode_lease_row(&row, status).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode persisted session lease failed: {error}"),
            )
        })?;
        let mut event = synapse_core::AgentEventRecord::new(
            super::agent_events::unix_time_ns_now(),
            synapse_core::AgentEventKind::LeaseAcquired,
        );
        event.session_id = Some(session_id.to_owned());
        event.attributes.conversation_id = Some(session_id.to_owned());
        event.payload = json!({
            "ttl_ms": status.ttl_ms,
            "lease_generation": status.generation,
            "lease_process_id": generation.process_id,
            "lease_process_started_at_unix_ms": generation.started_at_unix_ms,
        });
        let db = self.session_continuity_db()?;
        super::agent_events::record_agent_events_with_session_mutations(
            &db,
            &[],
            &[(session_lease_key(session_id), encoded)],
            &[event],
        )
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        tracing::info!(
            code = "MCP_SESSION_LEASE_ACQUIRE_COMMITTED",
            session_id,
            ttl_ms,
            expires_at_unix_ms = row.expires_at_unix_ms,
            owner_process_id = generation.process_id,
            "readback=CF_SESSIONS+CF_AGENT_EVENTS after=session_lease_acquire_committed"
        );
        Ok(())
    }

    /// Whether a persisted lifecycle row exists for `session_id`'s input lease.
    /// An action-acquired lease has none (#205).
    pub(super) fn session_lease_row_exists(&self, session_id: &str) -> Result<bool, ErrorData> {
        let db = self.session_continuity_db()?;
        cf_row_exists(&db, &session_lease_key(session_id))
            .map_err(|error| mcp_error(error.code(), error.to_string()))
    }

    pub(super) fn persist_session_lease_handoff(
        &self,
        from_session_id: &str,
        to_session_id: &str,
        status: &LeaseStatus,
    ) -> Result<LeaseHandoffContinuityReadback, ErrorData> {
        let ttl_ms = status.ttl_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist handoff input lease: missing ttl_ms",
            )
        })?;
        let expires_in_ms = status.expires_in_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist handoff input lease: missing expires_in_ms",
            )
        })?;
        if status.owner_session_id.as_deref() != Some(to_session_id) {
            return Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!(
                    "cannot persist input lease handoff to {to_session_id}: status owner is {:?}",
                    status.owner_session_id
                ),
            ));
        }
        let now = unix_ms_now();
        let generation = current_lease_process_generation();
        let row = PersistedSessionLease {
            schema_version: 1,
            session_id: to_session_id.to_owned(),
            stored_at_unix_ms: now,
            renewed_at_unix_ms: now.saturating_sub(status.renewed_at_ms_ago.unwrap_or_default()),
            ttl_ms,
            expires_at_unix_ms: now.saturating_add(expires_in_ms),
            owner_process_id: generation.process_id,
            owner_process_started_at_unix_ms: generation.started_at_unix_ms,
        };
        let encoded = lease_identity::encode_lease_row(&row, status).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode persisted session lease handoff failed: {error}"),
            )
        })?;
        let db = self.session_continuity_db()?;
        let from_key = session_lease_key(from_session_id);
        let to_key = session_lease_key(to_session_id);
        let from_row_existed_before = cf_row_exists(&db, &from_key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        db.mutate_batch_pressure_bypass(
            cf::CF_SESSIONS,
            [from_key.clone()],
            [(to_key.clone(), encoded)],
        )
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let from_row_exists_after = cf_row_exists(&db, &from_key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let to_row = db
            .scan_cf_prefix(cf::CF_SESSIONS, &to_key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?
            .into_iter()
            .find(|(row_key, _value)| row_key == &to_key);
        let to_row_exists_after = to_row.is_some();
        let to_row_session_id = to_row
            .map(|(_key, value)| {
                synapse_storage::decode_json::<PersistedSessionLease>(&value)
                    .map(|lease| lease.session_id)
                    .map_err(|error| {
                        mcp_error(
                            error.code(),
                            format!(
                                "decode persisted session lease handoff failed for {to_session_id}: {error}"
                            ),
                        )
                    })
            })
            .transpose()?;
        if from_row_exists_after
            || !to_row_exists_after
            || to_row_session_id.as_deref() != Some(to_session_id)
        {
            return Err(mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!(
                    "persisted session lease handoff readback mismatch from={from_session_id} to={to_session_id}: from_after={from_row_exists_after} to_after={to_row_exists_after} to_row_session_id={to_row_session_id:?}"
                ),
            ));
        }
        let readback = LeaseHandoffContinuityReadback {
            from_row_existed_before,
            from_row_exists_after,
            from_row_deleted: from_row_existed_before && !from_row_exists_after,
            to_row_exists_after,
            to_row_session_id,
        };
        tracing::info!(
            code = "MCP_SESSION_LEASE_HANDOFF_PERSISTED",
            from_session_id,
            to_session_id,
            readback = ?readback,
            ttl_ms,
            expires_at_unix_ms = row.expires_at_unix_ms,
            "readback=CF_SESSIONS after=session_lease_handoff_persisted"
        );
        Ok(readback)
    }

    /// Commits a lease handoff's source-row removal, recipient-row creation,
    /// and released/acquired journal pair atomically.
    pub(super) fn persist_session_lease_handoff_with_events(
        &self,
        from_session_id: &str,
        to_session_id: &str,
        status: &LeaseStatus,
    ) -> Result<LeaseHandoffContinuityReadback, ErrorData> {
        let ttl_ms = status.ttl_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist handoff input lease: missing ttl_ms",
            )
        })?;
        let expires_in_ms = status.expires_in_ms.ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "cannot persist handoff input lease: missing expires_in_ms",
            )
        })?;
        if status.owner_session_id.as_deref() != Some(to_session_id) {
            return Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!(
                    "cannot persist input lease handoff to {to_session_id}: status owner is {:?}",
                    status.owner_session_id
                ),
            ));
        }
        let now = unix_ms_now();
        let generation = current_lease_process_generation();
        let row = PersistedSessionLease {
            schema_version: 1,
            session_id: to_session_id.to_owned(),
            stored_at_unix_ms: now,
            renewed_at_unix_ms: now.saturating_sub(status.renewed_at_ms_ago.unwrap_or_default()),
            ttl_ms,
            expires_at_unix_ms: now.saturating_add(expires_in_ms),
            owner_process_id: generation.process_id,
            owner_process_started_at_unix_ms: generation.started_at_unix_ms,
        };
        let encoded = lease_identity::encode_lease_row(&row, status).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode persisted session lease handoff failed: {error}"),
            )
        })?;
        let db = self.session_continuity_db()?;
        let from_key = session_lease_key(from_session_id);
        let to_key = session_lease_key(to_session_id);
        let from_row_existed_before = cf_row_exists(&db, &from_key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        if !from_row_existed_before {
            return Err(mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!(
                    "cannot hand off input lease from {from_session_id}: source CF_SESSIONS row is absent"
                ),
            ));
        }
        let ts_ns = super::agent_events::unix_time_ns_now();
        let mut released =
            synapse_core::AgentEventRecord::new(ts_ns, synapse_core::AgentEventKind::LeaseReleased);
        released.session_id = Some(from_session_id.to_owned());
        released.reason_code = Some("handoff".to_owned());
        released.attributes.conversation_id = Some(from_session_id.to_owned());
        released.payload = json!({
            "to_session": to_session_id,
            "lease_generation": lease_identity::read_generation(&db, from_session_id)
                .map_err(|error| mcp_error(error_codes::STORAGE_CORRUPTED, error))?,
        });
        let mut acquired =
            synapse_core::AgentEventRecord::new(ts_ns, synapse_core::AgentEventKind::LeaseAcquired);
        acquired.session_id = Some(to_session_id.to_owned());
        acquired.reason_code = Some("handoff".to_owned());
        acquired.attributes.conversation_id = Some(to_session_id.to_owned());
        acquired.payload = json!({
            "from_session": from_session_id,
            "lease_generation": status.generation,
            "ttl_ms": status.ttl_ms,
            "lease_process_id": generation.process_id,
            "lease_process_started_at_unix_ms": generation.started_at_unix_ms,
        });
        super::agent_events::record_agent_events_with_session_mutations(
            &db,
            std::slice::from_ref(&from_key),
            &[(to_key, encoded)],
            &[released, acquired],
        )
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let readback = LeaseHandoffContinuityReadback {
            from_row_existed_before,
            from_row_exists_after: false,
            from_row_deleted: true,
            to_row_exists_after: true,
            to_row_session_id: Some(to_session_id.to_owned()),
        };
        tracing::info!(
            code = "MCP_SESSION_LEASE_HANDOFF_COMMITTED",
            from_session_id,
            to_session_id,
            readback = ?readback,
            ttl_ms,
            expires_at_unix_ms = row.expires_at_unix_ms,
            "readback=CF_SESSIONS+CF_AGENT_EVENTS after=session_lease_handoff_committed"
        );
        Ok(readback)
    }

    pub(super) fn restore_session_lease_if_needed(
        &self,
        session_id: &str,
    ) -> Result<(), ErrorData> {
        let Some(persisted) = self.read_persisted_session_lease(session_id)? else {
            return Ok(());
        };
        let current = lease::status();
        if persisted_lease_matches_physical_owner(&persisted, session_id, &current)
            && lease_identity::identity_matches(
                &*self.session_continuity_db()?,
                session_id,
                &current,
                true,
            )
            .map_err(|error| mcp_error(error_codes::STORAGE_CORRUPTED, error))?
        {
            tracing::info!(
                code = "MCP_SESSION_LEASE_PHYSICAL_OWNER_CONFIRMED",
                session_id,
                current_owner = ?current.owner_session_id,
                "persisted input lease matches the current process's physical owner"
            );
            return Ok(());
        }
        let _readback = self.reconcile_persisted_session_lease(
            session_id,
            &persisted,
            &current,
            LEASE_RECONCILIATION_REASON,
        )?;
        Ok(())
    }

    /// Reconciles all persisted lease rows before the daemon exposes a tool
    /// endpoint. A lease belongs to a process-local physical primitive, so an
    /// unclean/cold restart must close prior-process rows rather than recreate
    /// foreground authority from disk.
    pub(crate) fn reconcile_persisted_session_leases_on_startup(&self) -> Result<(), ErrorData> {
        let current = lease::status();
        let persisted = self.read_all_persisted_session_leases()?;
        let db = self.session_continuity_db()?;
        let mut preserved = 0_u32;
        let mut reconciled = 0_u32;
        for (row, generation) in persisted {
            if persisted_lease_matches_physical_owner(&row, &row.session_id, &current)
                && lease_identity::generation_matches(generation.as_deref(), &current, true)
            {
                preserved = preserved.saturating_add(1);
                continue;
            }
            let _readback = reconcile_persisted_session_lease_with_generation(
                &db,
                &row.session_id,
                &row,
                &current,
                LEASE_RECONCILIATION_REASON,
                generation.as_deref(),
            )
            .map_err(|error| mcp_error(error_codes::STORAGE_CORRUPTED, error))?;
            reconciled = reconciled.saturating_add(1);
        }
        tracing::info!(
            code = "MCP_SESSION_LEASE_STARTUP_RECONCILIATION",
            persisted_row_count = preserved.saturating_add(reconciled),
            preserved_row_count = preserved,
            reconciled_row_count = reconciled,
            physical_lease_held = current.held,
            physical_lease_owner = ?current.owner_session_id,
            "readback=CF_SESSIONS+CF_AGENT_EVENTS after=startup_lease_reconciliation"
        );
        Ok(())
    }

    pub(super) fn memory_session_target(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionTarget>, ErrorData> {
        let guard = self.session_targets_ref().lock().map_err(|_err| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session target registry lock poisoned",
            )
        })?;
        Ok(guard.get(session_id).cloned())
    }

    pub(super) fn persisted_session_target_read_model(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionTarget>, ErrorData> {
        Ok(self
            .read_persisted_session_target(session_id)?
            .map(|persisted| persisted.target))
    }

    fn read_persisted_session_target(
        &self,
        session_id: &str,
    ) -> Result<Option<PersistedSessionTarget>, ErrorData> {
        let key = session_target_key(session_id);
        let db = self.session_continuity_db()?;
        let rows = db
            .scan_cf_prefix(cf::CF_SESSIONS, &key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let Some((_row_key, value)) = rows.into_iter().find(|(row_key, _)| row_key == &key) else {
            return Ok(None);
        };
        let persisted =
            synapse_storage::decode_json::<PersistedSessionTarget>(&value).map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("decode persisted session target failed for {session_id}: {error}"),
                )
            })?;
        if persisted.schema_version != 1 || persisted.session_id != session_id {
            return Err(mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!(
                    "persisted session target row mismatch for {session_id}: schema_version={} row_session_id={}",
                    persisted.schema_version, persisted.session_id
                ),
            ));
        }
        Ok(Some(persisted))
    }

    fn read_persisted_session_lease(
        &self,
        session_id: &str,
    ) -> Result<Option<PersistedSessionLease>, ErrorData> {
        let key = session_lease_key(session_id);
        let db = self.session_continuity_db()?;
        let rows = db
            .scan_cf_prefix(cf::CF_SESSIONS, &key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let Some((_row_key, value)) = rows.into_iter().find(|(row_key, _)| row_key == &key) else {
            return Ok(None);
        };
        let persisted =
            synapse_storage::decode_json::<PersistedSessionLease>(&value).map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("decode persisted session lease failed for {session_id}: {error}"),
                )
            })?;
        if persisted.schema_version != 1 || persisted.session_id != session_id {
            return Err(mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!(
                    "persisted session lease row mismatch for {session_id}: schema_version={} row_session_id={}",
                    persisted.schema_version, persisted.session_id
                ),
            ));
        }
        Ok(Some(persisted))
    }

    fn read_all_persisted_session_leases(
        &self,
    ) -> Result<Vec<(PersistedSessionLease, Option<String>)>, ErrorData> {
        let db = self.session_continuity_db()?;
        let rows = db
            .scan_cf_prefix(cf::CF_SESSIONS, SESSION_LEASE_PREFIX.as_bytes())
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let mut persisted = Vec::with_capacity(rows.len());
        let warnings = skipped_rows::SkippedRowWarnings::startup_lease_rows();
        let mut skips = skipped_rows::SweepSkips::default();
        for (row_key, value) in rows {
            let row = lease_identity::decode_lease_row(&value);
            let valid = row.as_ref().is_ok_and(|(row, _generation)| {
                row.schema_version == 1 && row_key == session_lease_key(&row.session_id)
            });
            if !valid {
                if warnings.should_warn(&mut skips, &row_key) {
                    tracing::warn!(code = "MCP_SESSION_LEASE_STARTUP_ROW_SKIPPED",
                        key = ?row_key, "invalid lease row retained; continuing startup reconciliation");
                }
                continue;
            }
            if let Ok(row) = row {
                persisted.push(row);
            }
        }
        warnings.finish_sweep(skips);
        persisted.sort_by(|left, right| left.0.session_id.cmp(&right.0.session_id));
        Ok(persisted)
    }

    fn reconcile_persisted_session_lease(
        &self,
        session_id: &str,
        persisted: &PersistedSessionLease,
        physical: &LeaseStatus,
        reason_code: &str,
    ) -> Result<LeaseReleaseReconciliationReadback, ErrorData> {
        let db = self.session_continuity_db()?;
        reconcile_persisted_session_lease_from_db(&db, session_id, persisted, physical, reason_code)
            .map_err(|error| mcp_error(error_codes::STORAGE_CORRUPTED, error))
    }

    fn session_registry_read_for_persistence(
        &self,
        session_id: &str,
    ) -> Result<Option<super::session_registry::SessionRegistryRead>, ErrorData> {
        let now = unix_ms_now();
        let guard = self.session_registry_ref().lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session registry lock poisoned while reading CDP owner client identity",
            )
        })?;
        Ok(guard
            .reads(now)
            .into_iter()
            .find(|read| read.session_id == session_id))
    }

    fn session_continuity_db(&self) -> Result<Arc<Db>, ErrorData> {
        let mut state = self.m3_state.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "M3 service state lock poisoned while opening session continuity storage",
            )
        })?;
        state
            .ensure_storage()
            .map_err(|error| mcp_error(error.code(), error.to_string()))
    }
}

pub(crate) fn read_persisted_session_target_for_session(
    m3_state: &SharedM3State,
    session_id: &str,
) -> Result<Option<SessionTarget>, String> {
    let db = session_continuity_db_from_state(m3_state)?;
    read_persisted_session_target_from_db(&db, session_id).map(|row| row.map(|row| row.target))
}

fn read_persisted_session_target_from_db(
    db: &Db,
    session_id: &str,
) -> Result<Option<PersistedSessionTarget>, String> {
    let key = session_target_key(session_id);
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, &key)
        .map_err(|error| error.to_string())?;
    let Some((_row_key, value)) = rows.into_iter().find(|(row_key, _)| row_key == &key) else {
        return Ok(None);
    };
    let persisted =
        synapse_storage::decode_json::<PersistedSessionTarget>(&value).map_err(|error| {
            format!("decode persisted session target failed for {session_id}: {error}")
        })?;
    if persisted.schema_version != 1 || persisted.session_id != session_id {
        return Err(format!(
            "persisted session target row mismatch for {session_id}: schema_version={} row_session_id={}",
            persisted.schema_version, persisted.session_id
        ));
    }
    Ok(Some(persisted))
}

pub(crate) fn delete_persisted_session_target_row(
    m3_state: &SharedM3State,
    session_id: &str,
) -> Result<TargetContinuityCleanupReadback, String> {
    let db = session_continuity_db_from_state(m3_state)?;
    delete_persisted_session_target_row_from_db(&db, session_id)
}

pub(crate) fn delete_persisted_session_target_row_from_db(
    db: &Db,
    session_id: &str,
) -> Result<TargetContinuityCleanupReadback, String> {
    let key = session_target_key(session_id);
    let row_existed_before = cf_row_exists(&db, &key).map_err(|error| error.to_string())?;
    db.delete_batch(cf::CF_SESSIONS, [key.clone()])
        .map_err(|error| error.to_string())?;
    let row_exists_after = cf_row_exists(&db, &key).map_err(|error| error.to_string())?;
    if row_exists_after {
        return Err(format!(
            "session target continuity row still exists after delete for {session_id}"
        ));
    }
    Ok(TargetContinuityCleanupReadback {
        row_existed_before,
        row_exists_after,
        row_deleted: row_existed_before,
    })
}

fn read_persisted_session_lease_from_db(
    db: &Db,
    session_id: &str,
) -> Result<Option<PersistedSessionLease>, String> {
    let key = session_lease_key(session_id);
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, &key)
        .map_err(|error| error.to_string())?;
    let Some((_row_key, value)) = rows.into_iter().find(|(row_key, _)| row_key == &key) else {
        return Ok(None);
    };
    let persisted =
        synapse_storage::decode_json::<PersistedSessionLease>(&value).map_err(|error| {
            format!("decode persisted session lease failed for {session_id}: {error}")
        })?;
    if persisted.schema_version != 1 || persisted.session_id != session_id {
        return Err(format!(
            "persisted session lease row mismatch for {session_id}: schema_version={} row_session_id={}",
            persisted.schema_version, persisted.session_id
        ));
    }
    Ok(Some(persisted))
}

/// Closes an open persisted input-lease lifecycle row and writes its balancing
/// `LeaseReleased` event in one durable transition. The row is the exactly-once
/// marker: callers that arrive after a prior closer observe no row and emit no
/// duplicate release event.
pub(crate) fn reconcile_persisted_session_lease_row(
    m3_state: &SharedM3State,
    session_id: &str,
    reason_code: &str,
) -> Result<LeaseReleaseReconciliationReadback, String> {
    let db = session_continuity_db_from_state(m3_state)?;
    let Some(persisted) = read_persisted_session_lease_from_db(&db, session_id)? else {
        return Ok(LeaseReleaseReconciliationReadback {
            session_id: session_id.to_owned(),
            reason_code: reason_code.to_owned(),
            ..LeaseReleaseReconciliationReadback::default()
        });
    };
    let physical = lease::status();
    reconcile_persisted_session_lease_from_db(&db, session_id, &persisted, &physical, reason_code)
}

/// Emergency reports can be replayed; never close another acquisition's row.
pub(crate) fn reconcile_displaced_session_lease_row(
    m3_state: &SharedM3State,
    displaced: &LeaseStatus,
    reason_code: &str,
) -> Result<LeaseReleaseReconciliationReadback, String> {
    let session_id = displaced.owner_session_id.as_deref().unwrap_or_default();
    let db = session_continuity_db_from_state(m3_state)?;
    if session_id.is_empty()
        || !lease_identity::identity_matches(&db, session_id, displaced, false)?
    {
        return Ok(LeaseReleaseReconciliationReadback {
            session_id: session_id.to_owned(),
            reason_code: reason_code.to_owned(),
            ..LeaseReleaseReconciliationReadback::default()
        });
    }
    let Some(persisted) = read_persisted_session_lease_from_db(&db, session_id)? else {
        return Ok(LeaseReleaseReconciliationReadback::default());
    };
    reconcile_persisted_session_lease_from_db(
        &db,
        session_id,
        &persisted,
        &lease::status(),
        reason_code,
    )
}

/// Closes every persisted input-lease row whose lease no longer physically
/// exists in this process (expired, preempted, force-cleared, left by a prior
/// process), journaling one `LeaseReleased` per row. The row is the
/// exactly-once marker, so this is idempotent and is the retry path for any
/// release whose journal write was skipped or failed (#93/#94/#95). Rows that
/// match the live physical owner are left alone. Callers hold the lease
/// transition gate so an in-flight acquire (physical lease, then row) is never
/// closed under it. Undecodable rows are skipped and logged.
pub(crate) fn reconcile_orphaned_session_lease_rows(
    m3_state: &SharedM3State,
    reason_code: &str,
) -> Result<u32, String> {
    reconcile_orphaned_session_lease_rows_with_warnings(
        m3_state,
        reason_code,
        skipped_rows::SkippedRowWarnings::global(),
    )
}

/// [`reconcile_orphaned_session_lease_rows`] with an explicit warn-once memory
/// instead of the process-wide one (test seam: isolated #180 bookkeeping).
fn reconcile_orphaned_session_lease_rows_with_warnings(
    m3_state: &SharedM3State,
    reason_code: &str,
    skip_log: &skipped_rows::SkippedRowWarnings,
) -> Result<u32, String> {
    let physical = lease::status();
    // While the operator holds the lease (panic hotkey K1 -> K2 window), the
    // displaced owner's row belongs to K2, which journals the fixed
    // `operator_hotkey_preempted` reason. Sweeping now would steal that reason
    // (#94); K2 clears the operator lease and the next tick resumes.
    if lease::emergency_cleanup_pending()
        || (physical.held
            && physical.owner_session_id.as_deref()
                == Some(synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID))
    {
        return Ok(0);
    }
    let db = session_continuity_db_from_state(m3_state)?;
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, SESSION_LEASE_PREFIX.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut closed = 0_u32;
    let mut sweep_skips = skipped_rows::SweepSkips::default();
    for (row_key, value) in rows {
        let (persisted, generation) = match lease_identity::decode_lease_row(&value) {
            Ok((row, generation))
                if row.schema_version == 1 && row_key == session_lease_key(&row.session_id) =>
            {
                (row, generation)
            }
            Ok(_) | Err(_) => {
                // #180: the sweep ticks every 250 ms; warn once per corrupt key.
                if skip_log.should_warn(&mut sweep_skips, &row_key) {
                    tracing::warn!(
                        code = "MCP_SESSION_LEASE_ORPHAN_SWEEP_ROW_SKIPPED",
                        key = %String::from_utf8_lossy(&row_key),
                        "persisted session lease row is undecodable or mismatched; left for repair (reported once per key)"
                    );
                }
                continue;
            }
        };
        if persisted_lease_matches_physical_owner(&persisted, &persisted.session_id, &physical)
            && lease_identity::generation_matches(generation.as_deref(), &physical, true)
        {
            continue;
        }
        let session_id = persisted.session_id.clone();
        match reconcile_persisted_session_lease_with_generation(
            &db,
            &session_id,
            &persisted,
            &physical,
            reason_code,
            generation.as_deref(),
        ) {
            Ok(readback) if readback.release_event_written => closed = closed.saturating_add(1),
            Ok(_) => {}
            Err(error) => tracing::error!(
                code = "MCP_SESSION_LEASE_ORPHAN_SWEEP_FAILED",
                session_id = %session_id,
                error = %error,
                "orphaned session lease row could not be closed; the next sweep retries"
            ),
        }
    }
    skip_log.finish_sweep(sweep_skips);
    Ok(closed)
}

fn reconcile_persisted_session_lease_from_db(
    db: &Db,
    session_id: &str,
    persisted: &PersistedSessionLease,
    physical: &LeaseStatus,
    reason_code: &str,
) -> Result<LeaseReleaseReconciliationReadback, String> {
    let generation = lease_identity::read_generation(db, session_id)?;
    reconcile_persisted_session_lease_with_generation(
        db,
        session_id,
        persisted,
        physical,
        reason_code,
        generation.as_deref(),
    )
}

fn reconcile_persisted_session_lease_with_generation(
    db: &Db,
    session_id: &str,
    persisted: &PersistedSessionLease,
    physical: &LeaseStatus,
    reason_code: &str,
    generation: Option<&str>,
) -> Result<LeaseReleaseReconciliationReadback, String> {
    let key = session_lease_key(session_id);
    let row_existed_before = cf_row_exists(db, &key).map_err(|error| error.to_string())?;
    if !row_existed_before {
        return Ok(LeaseReleaseReconciliationReadback {
            session_id: session_id.to_owned(),
            reason_code: reason_code.to_owned(),
            ..LeaseReleaseReconciliationReadback::default()
        });
    }
    let mut released = synapse_core::AgentEventRecord::new(
        super::agent_events::unix_time_ns_now(),
        synapse_core::AgentEventKind::LeaseReleased,
    );
    released.session_id = Some(session_id.to_owned());
    released.reason_code = Some(reason_code.to_owned());
    released.attributes.conversation_id = Some(session_id.to_owned());
    released.payload = json!({
        "lease_generation": generation,
        "persisted_expires_at_unix_ms": persisted.expires_at_unix_ms,
        "persisted_owner_process_id": persisted.owner_process_id,
        "persisted_owner_process_started_at_unix_ms": persisted.owner_process_started_at_unix_ms,
        "physical_lease_held": physical.held,
        "physical_lease_owner_session_id": physical.owner_session_id,
        "physical_lease_ttl_ms": physical.ttl_ms,
    });
    super::agent_events::record_agent_events_with_session_mutations(
        db,
        std::slice::from_ref(&key),
        &[],
        &[released],
    )
    .map_err(|error| error.to_string())?;
    let row_exists_after = cf_row_exists(db, &key).map_err(|error| error.to_string())?;
    if row_exists_after {
        return Err(format!(
            "session lease continuity row still exists after reconciled release for {session_id}"
        ));
    }
    let readback = LeaseReleaseReconciliationReadback {
        session_id: session_id.to_owned(),
        row_existed_before,
        row_exists_after,
        row_deleted: true,
        release_event_written: true,
        reason_code: reason_code.to_owned(),
    };
    tracing::warn!(
        code = "MCP_SESSION_LEASE_RECONCILED",
        session_id,
        reason_code,
        persisted_owner_process_id = persisted.owner_process_id,
        physical_lease_held = physical.held,
        physical_lease_owner = ?physical.owner_session_id,
        readback = ?readback,
        "readback=CF_SESSIONS+CF_AGENT_EVENTS after=session_lease_reconciled"
    );
    Ok(readback)
}

fn persisted_lease_matches_physical_owner(
    persisted: &PersistedSessionLease,
    session_id: &str,
    physical: &LeaseStatus,
) -> bool {
    let generation = current_lease_process_generation();
    physical.held
        && physical.owner_session_id.as_deref() == Some(session_id)
        && persisted.owner_process_id == generation.process_id
        && persisted.owner_process_started_at_unix_ms == generation.started_at_unix_ms
}

fn validate_restored_target(session_id: &str, target: &SessionTarget) -> Result<(), ErrorData> {
    match target {
        SessionTarget::Window { hwnd } => validate_target_window(*hwnd).map(|_| ()),
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => {
            if cdp_target_id.trim().is_empty() {
                return Err(mcp_error(
                    error_codes::TARGET_CDP_UNRESOLVED,
                    format!(
                        "persisted CDP session target for {session_id} has an empty cdp_target_id"
                    ),
                ));
            }
            validate_target_window(*window_hwnd).map(|_| ())
        }
    }
}

fn delete_exact_session_row(db: &Db, key: Vec<u8>) -> Result<(), ErrorData> {
    db.delete_batch(cf::CF_SESSIONS, [key])
        .map_err(|error| mcp_error(error.code(), error.to_string()))
}

fn cf_row_exists(db: &Db, key: &[u8]) -> synapse_storage::StorageResult<bool> {
    db.scan_cf_prefix(cf::CF_SESSIONS, key).map(|rows| {
        rows.into_iter()
            .any(|(row_key, _value)| row_key.as_slice() == key)
    })
}

pub(crate) fn delete_persisted_cdp_target_owner_row(
    m3_state: &SharedM3State,
    owner_key: &str,
    cdp_target_id: &str,
) -> Result<bool, String> {
    let db = session_continuity_db_from_state(m3_state)?;
    delete_persisted_cdp_target_owner_from_db(&db, owner_key, cdp_target_id)
}

pub(crate) fn read_persisted_cdp_target_owners_for_session(
    m3_state: &SharedM3State,
    session_id: &str,
) -> Result<Vec<(String, PersistedCdpTargetOwner)>, String> {
    let db = session_continuity_db_from_state(m3_state)?;
    read_persisted_cdp_target_owners_for_session_from_db(&db, session_id)
}

pub(crate) fn persisted_session_target_session_ids(
    m3_state: &SharedM3State,
) -> Result<BTreeSet<String>, String> {
    let db = session_continuity_db_from_state(m3_state)?;
    read_persisted_session_target_session_ids_from_db(&db)
}

pub(crate) fn persisted_cdp_target_owner_session_ids(
    m3_state: &SharedM3State,
) -> Result<BTreeSet<String>, String> {
    let db = session_continuity_db_from_state(m3_state)?;
    read_persisted_cdp_target_owner_session_ids_from_db(&db)
}

pub(crate) fn persisted_cdp_target_owner_row_key_string(
    owner_key: &str,
    cdp_target_id: &str,
) -> String {
    String::from_utf8_lossy(&cdp_target_owner_row_key(owner_key, cdp_target_id)).into_owned()
}

fn delete_persisted_cdp_target_owner_from_db(
    db: &Db,
    owner_key: &str,
    cdp_target_id: &str,
) -> Result<bool, String> {
    let key = cdp_target_owner_row_key(owner_key, cdp_target_id);
    let existed_before = cf_row_exists(db, &key).map_err(|error| error.to_string())?;
    db.delete_batch(cf::CF_SESSIONS, [key.clone()])
        .map_err(|error| error.to_string())?;
    let exists_after = cf_row_exists(db, &key).map_err(|error| error.to_string())?;
    if exists_after {
        return Err(format!(
            "persisted CDP target owner row still exists after delete owner_key={owner_key:?} cdp_target_id={cdp_target_id:?}"
        ));
    }
    tracing::info!(
        code = "MCP_SESSION_CDP_TARGET_OWNER_DELETED",
        owner_key = %owner_key,
        cdp_target_id = %cdp_target_id,
        existed_before,
        "readback=CF_SESSIONS after=cdp_target_owner_deleted"
    );
    Ok(existed_before)
}

fn read_persisted_cdp_target_owners_for_session_from_db(
    db: &Db,
    session_id: &str,
) -> Result<Vec<(String, PersistedCdpTargetOwner)>, String> {
    read_persisted_cdp_target_owners_for_session_with_warnings(
        db,
        session_id,
        skipped_rows::SkippedRowWarnings::cdp_owner_rows(),
    )
}

fn read_persisted_cdp_target_owners_for_session_with_warnings(
    db: &Db,
    session_id: &str,
    skip_log: &skipped_rows::SkippedRowWarnings,
) -> Result<Vec<(String, PersistedCdpTargetOwner)>, String> {
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, SESSION_CDP_TARGET_OWNER_PREFIX.as_bytes())
        .map_err(|error| error.to_string())?;
    // #216: one bad row (another build's schema, a torn write) must not fail
    // the scan for every unrelated session. Rows of other sessions are skipped
    // before strict validation; a bad row is skipped, warned once, and left in
    // place for repair.
    let mut skips = skipped_rows::SweepSkips::default();
    let mut decoded = Vec::new();
    for (row_key, value) in rows {
        let row = match synapse_storage::decode_json::<PersistedCdpTargetOwner>(&value) {
            Ok(row) => row,
            Err(error) => {
                warn_skipped_cdp_owner_row(
                    skip_log,
                    &mut skips,
                    &row_key,
                    &format!("decode persisted CDP target owner failed: {error}"),
                );
                continue;
            }
        };
        if row.owner_session_id != session_id {
            continue;
        }
        if let Err(reason) = check_persisted_cdp_target_owner_row(&row_key, &row) {
            warn_skipped_cdp_owner_row(skip_log, &mut skips, &row_key, &reason);
            continue;
        }
        decoded.push((row.owner_key.clone(), row));
    }
    // A per-session scan sees only this session's rows strictly, so it must not
    // prune the shared memory: that would forget another session's bad row and
    // make the next full scan warn again (#216). The full session-id scan prunes.
    drop(skips);
    decoded.sort_by(|left, right| {
        left.1
            .owner
            .cdp_target_id
            .cmp(&right.1.owner.cdp_target_id)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(decoded)
}

fn read_persisted_session_target_session_ids_from_db(db: &Db) -> Result<BTreeSet<String>, String> {
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, SESSION_TARGET_PREFIX.as_bytes())
        .map_err(|error| error.to_string())?;
    // #216: a bad row is skipped and warned once, never an error for the scan.
    let skip_log = skipped_rows::SkippedRowWarnings::session_target_rows();
    let mut skips = skipped_rows::SweepSkips::default();
    let mut session_ids = BTreeSet::new();
    for (row_key, value) in rows {
        let checked = session_id_from_target_row_key(&row_key).and_then(|key_session_id| {
            let row = synapse_storage::decode_json::<PersistedSessionTarget>(&value)
                .map_err(|error| format!("decode persisted session target failed: {error}"))?;
            if row.schema_version != 1 || row.session_id != key_session_id {
                return Err(format!(
                    "persisted session target row mismatch: row_key={} schema_version={} row_session_id={}",
                    String::from_utf8_lossy(&row_key),
                    row.schema_version,
                    row.session_id
                ));
            }
            Ok(key_session_id)
        });
        match checked {
            Ok(key_session_id) => {
                session_ids.insert(key_session_id);
            }
            Err(reason) => {
                if skip_log.should_warn(&mut skips, &row_key) {
                    tracing::warn!(
                        code = "MCP_SESSION_TARGET_ROW_SKIPPED",
                        key = %String::from_utf8_lossy(&row_key),
                        detail = %reason,
                        "persisted session target row is undecodable or mismatched; left for repair (reported once per key)"
                    );
                }
            }
        }
    }
    skip_log.finish_sweep(skips);
    Ok(session_ids)
}

fn read_persisted_cdp_target_owner_session_ids_from_db(
    db: &Db,
) -> Result<BTreeSet<String>, String> {
    read_persisted_cdp_target_owner_session_ids_with_warnings(
        db,
        skipped_rows::SkippedRowWarnings::cdp_owner_rows(),
    )
}

fn read_persisted_cdp_target_owner_session_ids_with_warnings(
    db: &Db,
    skip_log: &skipped_rows::SkippedRowWarnings,
) -> Result<BTreeSet<String>, String> {
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, SESSION_CDP_TARGET_OWNER_PREFIX.as_bytes())
        .map_err(|error| error.to_string())?;
    // #216: a bad row is skipped and warned once; it must not hide every
    // other session from the stale-candidate scan on each 250 ms tick.
    let mut skips = skipped_rows::SweepSkips::default();
    let mut session_ids = BTreeSet::new();
    for (row_key, value) in rows {
        let row = match synapse_storage::decode_json::<PersistedCdpTargetOwner>(&value) {
            Ok(row) => row,
            Err(error) => {
                warn_skipped_cdp_owner_row(
                    skip_log,
                    &mut skips,
                    &row_key,
                    &format!("decode persisted CDP target owner failed: {error}"),
                );
                continue;
            }
        };
        if let Err(reason) = check_persisted_cdp_target_owner_row(&row_key, &row) {
            warn_skipped_cdp_owner_row(skip_log, &mut skips, &row_key, &reason);
            continue;
        }
        session_ids.insert(row.owner_session_id);
    }
    skip_log.finish_sweep(skips);
    Ok(session_ids)
}

/// Strict validation of one decoded persisted CDP owner row against its key.
fn check_persisted_cdp_target_owner_row(
    row_key: &[u8],
    row: &PersistedCdpTargetOwner,
) -> Result<(), String> {
    validate_persisted_cdp_target_owner(&row.owner.cdp_target_id, row)
        .map_err(|error| error.message.to_string())?;
    let expected_key = cdp_target_owner_row_key(&row.owner_key, &row.owner.cdp_target_id);
    if row_key != expected_key {
        return Err(format!(
            "persisted CDP target owner row key mismatch: row_key={} owner_key={}",
            String::from_utf8_lossy(row_key),
            row.owner_key
        ));
    }
    Ok(())
}

fn warn_skipped_cdp_owner_row(
    skip_log: &skipped_rows::SkippedRowWarnings,
    skips: &mut skipped_rows::SweepSkips,
    row_key: &[u8],
    reason: &str,
) {
    if skip_log.should_warn(skips, row_key) {
        tracing::warn!(
            code = "MCP_SESSION_CDP_OWNER_ROW_SKIPPED",
            key = %String::from_utf8_lossy(row_key),
            detail = %reason,
            "persisted CDP target owner row is undecodable or mismatched; left for repair (reported once per key)"
        );
    }
}

fn session_id_from_target_row_key(row_key: &[u8]) -> Result<String, String> {
    let key = std::str::from_utf8(row_key)
        .map_err(|error| format!("persisted session target row key is not UTF-8: {error}"))?;
    let Some(session_id) = key.strip_prefix(SESSION_TARGET_PREFIX) else {
        return Err(format!(
            "persisted session target row key has unexpected prefix: {key}"
        ));
    };
    if session_id.is_empty()
        || session_id.chars().count() > 512
        || !session_id.chars().all(|ch| ('!'..='~').contains(&ch))
    {
        return Err(format!(
            "persisted session target row key has invalid session id: {key}"
        ));
    }
    Ok(session_id.to_owned())
}

fn validate_persisted_cdp_target_owner(
    requested_cdp_target_id: &str,
    row: &PersistedCdpTargetOwner,
) -> Result<(), ErrorData> {
    if row.schema_version != 1 {
        return Err(mcp_error(
            error_codes::STORAGE_CORRUPTED,
            format!(
                "persisted CDP target owner row has unsupported schema_version={}",
                row.schema_version
            ),
        ));
    }
    if row.owner_key.trim().is_empty() {
        return Err(mcp_error(
            error_codes::STORAGE_CORRUPTED,
            "persisted CDP target owner row has empty owner_key",
        ));
    }
    if row.owner_session_id != row.owner.session_id {
        return Err(mcp_error(
            error_codes::STORAGE_CORRUPTED,
            format!(
                "persisted CDP target owner session mismatch: row_session_id={} owner_session_id={}",
                row.owner_session_id, row.owner.session_id
            ),
        ));
    }
    if normalize_cdp_target_id(&row.owner.cdp_target_id)
        != normalize_cdp_target_id(requested_cdp_target_id)
    {
        return Err(mcp_error(
            error_codes::STORAGE_CORRUPTED,
            format!(
                "persisted CDP target owner target mismatch: requested={requested_cdp_target_id:?} row={:?}",
                row.owner.cdp_target_id
            ),
        ));
    }
    Ok(())
}

fn cdp_target_owner_target_prefix(cdp_target_id: &str) -> Vec<u8> {
    let normalized = normalize_cdp_target_id(cdp_target_id);
    format!(
        "{SESSION_CDP_TARGET_OWNER_PREFIX}{}:{normalized}/",
        normalized.len()
    )
    .into_bytes()
}

fn cdp_target_owner_row_key(owner_key: &str, cdp_target_id: &str) -> Vec<u8> {
    format!(
        "{}{}",
        String::from_utf8_lossy(&cdp_target_owner_target_prefix(cdp_target_id)),
        owner_key
    )
    .into_bytes()
}

fn normalize_cdp_target_id(cdp_target_id: &str) -> String {
    cdp_target_id.trim().to_ascii_lowercase()
}

fn session_continuity_db_from_state(m3_state: &SharedM3State) -> Result<Arc<Db>, String> {
    let mut state = m3_state.lock().map_err(|_error| {
        "M3 service state lock poisoned while opening session continuity storage".to_owned()
    })?;
    state
        .ensure_storage()
        .map_err(|error| format!("open storage for session continuity cleanup: {error}"))
}

fn session_target_key(session_id: &str) -> Vec<u8> {
    format!("{SESSION_TARGET_PREFIX}{session_id}").into_bytes()
}

fn session_lease_key(session_id: &str) -> Vec<u8> {
    format!("{SESSION_LEASE_PREFIX}{session_id}").into_bytes()
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn current_lease_process_generation() -> LeaseProcessGeneration {
    *LEASE_PROCESS_GENERATION.get_or_init(|| LeaseProcessGeneration {
        process_id: std::process::id(),
        started_at_unix_ms: unix_ms_now(),
    })
}

#[path = "session_continuity_skipped_rows.rs"]
mod skipped_rows;

#[cfg(test)]
#[path = "tests/session_lease_skip_warn_once_180.rs"]
mod session_lease_skip_warn_once_180_tests;

#[cfg(test)]
#[path = "tests/session_lease_orphan_sweep.rs"]
mod session_lease_orphan_sweep_tests;

#[cfg(test)]
#[path = "tests/session_lease_hotkey_sweep_94.rs"]
mod session_lease_hotkey_sweep_94_tests;

#[cfg(test)]
#[path = "tests/session_lease_restart_reconcile_95.rs"]
mod session_lease_restart_reconcile_95_tests;

#[cfg(test)]
#[path = "tests/session_cdp_owner_bad_row_216.rs"]
mod session_cdp_owner_bad_row_216_tests;

#[cfg(test)]
#[path = "tests/journal_failure_retry_93.rs"]
mod journal_failure_retry_93;
#[cfg(test)]
#[path = "tests/orphan_warning_events_180.rs"]
mod orphan_warning_events_180;
#[cfg(test)]
#[path = "tests/startup_corrupt_leases_216.rs"]
mod startup_corrupt_leases_216;

#[cfg(test)]
#[path = "tests/durable_lease_generation_95.rs"]
mod durable_lease_generation_95;
