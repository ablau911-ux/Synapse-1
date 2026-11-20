//! Persisted routine identity owners (#165).
//!
//! A routine's dependent rows outlive it: when mining stops deriving a
//! routine, its `CF_ROUTINE_STATE`, `armed_routine/v1`,
//! `routine_automation/v1`, and `hygiene/taint/v1/routine/` rows stay under
//! its id. `CF_KV` rows `routine_identity/v1/<routine_id>` record the raw
//! steps that own each id, so a later mining pass hands a retained id only to
//! a routine with exactly those raw steps (a returning routine). A newcomer
//! whose fresh id collides gets a derived id and inherits nothing
//! ([`synapse_core::routine_identity::reconcile_routine_identities_retained`]).
//! An id with rows but no recorded owner is reserved outright.

use std::collections::{BTreeMap, BTreeSet};

use rmcp::ErrorData;
use synapse_core::error_codes;
use synapse_core::retention::ROUTINE_IDENTITY_MAX_BYTES;
use synapse_core::routine_identity::{RetainedIdentities, RoutineIdentityOwner};
use synapse_core::types::RoutineRecord;
use synapse_storage::{Db, StorageError, cf, decode_json, encode_json};

use crate::m1::mcp_error;

use super::armed_routines::{armed_routine_key, load_all_armed_routines};
use super::hygiene::taint_key_prefix;
use super::profile_authoring::ROUTINE_AUTOMATION_PREFIX;
use super::routines::load_all_state_rows;

#[cfg(test)]
#[path = "tests/routine_ledger_admission_hook_165.rs"]
pub(crate) mod admission_test_hook;

/// `CF_KV` prefix of the identity owner rows.
pub const ROUTINE_IDENTITY_PREFIX: &str = "routine_identity/v1/";

fn ledger_key(routine_id: &str) -> Vec<u8> {
    format!("{ROUTINE_IDENTITY_PREFIX}{routine_id}").into_bytes()
}

fn read_error(error: impl std::fmt::Display) -> ErrorData {
    mcp_error(
        error_codes::STORAGE_READ_FAILED,
        format!("ROUTINE_IDENTITY_READ_FAILED: {error}"),
    )
}

fn write_error(error: StorageError) -> ErrorData {
    mcp_error(
        error.code(),
        format!("ROUTINE_IDENTITY_WRITE_FAILED: {error}"),
    )
}

/// Recorded owners plus every id that still owns dependent rows, read once
/// per mining pass before identities are reconciled.
pub(super) struct RetainedIdentityRows {
    ledger: BTreeMap<String, RoutineIdentityOwner>,
    retained: RetainedIdentities,
}

impl RetainedIdentityRows {
    /// Reads the owner ledger and the ids of all dependent rows.
    pub(super) fn load(db: &std::sync::Arc<Db>) -> Result<Self, ErrorData> {
        let ledger = load_ledger(db)?;
        let row_ids = dependent_row_ids(db)?;
        let mut retained: RetainedIdentities = ledger
            .iter()
            .map(|(id, owner)| (id.clone(), Some(owner.clone())))
            .collect();
        for id in &row_ids {
            retained.entry(id.clone()).or_insert(None);
        }
        Ok(Self { ledger, retained })
    }

    /// Retained ids with their recorded owners, for reconciliation.
    pub(super) const fn retained(&self) -> &RetainedIdentities {
        &self.retained
    }

    /// Ownership is permanent, even after every dependent row is removed.
    /// Only schedule metadata can change on a recorded owner. A pre-ledger
    /// alias may bootstrap its mined spelling once, before ownership exists.
    ///
    /// One atomic, pressure-checked batch, refused whole when it would exceed
    /// the byte cap. Mining calls this before any other mutation.
    pub(super) fn persist(
        &self,
        db: &Db,
        previous: &[RoutineRecord],
        mined: &[RoutineRecord],
    ) -> Result<(), ErrorData> {
        let puts = self.pending_puts(previous, mined)?;
        if puts.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        admission_test_hook::run(db);
        check_ledger_capacity(db, &puts)?;
        db.put_batch(cf::CF_KV, puts).map_err(write_error)?;
        // Preserve the old synchronous ledger durability before replacing routines.
        db.flush().map_err(write_error)
    }

    fn pending_puts(
        &self,
        previous: &[RoutineRecord],
        mined: &[RoutineRecord],
    ) -> Result<Vec<LedgerPut>, ErrorData> {
        validate_identity_records(previous)?;
        validate_identity_records(mined)?;
        let owners = self.pending_owners(previous, mined)?;
        let mut puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (id, owner) in &owners {
            if self.ledger.get(*id) != Some(owner) {
                puts.push((ledger_key(id), encode_json(owner).map_err(write_error)?));
            }
        }
        Ok(puts)
    }

    fn pending_owners<'a>(
        &self,
        previous: &'a [RoutineRecord],
        mined: &'a [RoutineRecord],
    ) -> Result<BTreeMap<&'a str, RoutineIdentityOwner>, ErrorData> {
        let mut owners = BTreeMap::new();
        for routine in previous {
            let recoverable = self
                .ledger
                .get(&routine.routine_id)
                .is_none_or(|owner| owner.schedule.is_none() && owner.owns_steps(routine));
            if recoverable {
                owners.insert(
                    routine.routine_id.as_str(),
                    RoutineIdentityOwner::of(routine),
                );
            }
        }
        for routine in mined {
            if self
                .ledger
                .get(&routine.routine_id)
                .is_some_and(|owner| !owner.owns_steps(routine))
            {
                return Err(corrupt(format!(
                    "ownership reassignment refused for {}",
                    routine.routine_id
                )));
            }
            owners.insert(
                routine.routine_id.as_str(),
                RoutineIdentityOwner::of(routine),
            );
        }
        Ok(owners)
    }
}

type LedgerPut = (Vec<u8>, Vec<u8>);

/// Count logical key + value bytes, including net replacement growth and
/// schedule metadata. Refuse growth; never evict an owner (ids stay reserved).
/// Mining serializes ledger writers; re-read here so repeated persists stay exact.
fn check_ledger_capacity(db: &Db, puts: &[LedgerPut]) -> Result<(), ErrorData> {
    let rows = db
        .scan_cf_prefix(cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes())
        .map_err(read_error)?;
    let mut sizes: BTreeMap<_, _> = rows
        .into_iter()
        .map(|(key, value)| {
            let bytes = key.len().saturating_add(value.len());
            (key, bytes)
        })
        .collect();
    for (key, value) in puts {
        sizes.insert(key.clone(), key.len().saturating_add(value.len()));
    }
    let bytes = sizes
        .values()
        .fold(0usize, |sum, bytes| sum.saturating_add(*bytes));
    if bytes > ROUTINE_IDENTITY_MAX_BYTES {
        return Err(mcp_error(
            error_codes::STORAGE_CF_HARD_CAP_REACHED,
            format!(
                "routine_identity ledger would use {bytes} bytes; cap is {ROUTINE_IDENTITY_MAX_BYTES}; no owners evicted"
            ),
        ));
    }
    Ok(())
}

fn corrupt(detail: impl std::fmt::Display) -> ErrorData {
    mcp_error(
        error_codes::STORAGE_CORRUPTED,
        format!("ROUTINE_IDENTITY_ROW_INVALID: {detail}"),
    )
}

/// Corrupt evidence must stop mining before any migration, deletion or write.
fn load_ledger(db: &Db) -> Result<BTreeMap<String, RoutineIdentityOwner>, ErrorData> {
    let rows = db
        .scan_cf_prefix(cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes())
        .map_err(read_error)?;
    let mut ledger = BTreeMap::new();
    for (key, value) in &rows {
        let owner = decode_json::<RoutineIdentityOwner>(value).map_err(corrupt)?;
        validate_owner(key, &owner)?;
        ledger.insert(owner.routine_id.clone(), owner);
    }
    Ok(ledger)
}

fn validate_owner(key: &[u8], owner: &RoutineIdentityOwner) -> Result<(), ErrorData> {
    synapse_storage::routines::routine_key(&owner.routine_id).map_err(corrupt)?;
    let invalid_schedule = owner.schedule.is_some_and(|schedule| {
        schedule.mean_minute_of_day >= 1440 || schedule.tolerance_minutes > 720
    });
    if key != ledger_key(&owner.routine_id) || owner.steps.is_empty() || invalid_schedule {
        return Err(corrupt(format!(
            "invalid identity evidence at {}",
            String::from_utf8_lossy(key)
        )));
    }
    Ok(())
}

/// Current records can seed a ledger during upgrade. Validate them before
/// matching or any migration write, including when the new source is empty.
pub(super) fn validate_identity_records(records: &[RoutineRecord]) -> Result<(), ErrorData> {
    for record in records {
        validate_owner(
            &ledger_key(&record.routine_id),
            &RoutineIdentityOwner::of(record),
        )?;
    }
    Ok(())
}

/// Ids of every state, armed, automation, and routine taint row.
fn dependent_row_ids(db: &std::sync::Arc<Db>) -> Result<BTreeSet<String>, ErrorData> {
    // Keep existing fail-closed validation before writes. Also reserve armed
    // physical keys: a decoded payload naming another id cannot free its key.
    let mut scanned = 0;
    let mut ids: BTreeSet<_> = load_all_state_rows(db, &mut scanned)?.into_keys().collect();
    ids.extend(
        load_all_armed_routines(db)?
            .into_iter()
            .map(|row| row.routine_id),
    );
    let armed_prefix = armed_routine_key("");
    let taint_prefix = taint_key_prefix("routine");
    for (cf_name, prefix) in [
        (cf::CF_KV, armed_prefix.as_bytes()),
        (cf::CF_KV, ROUTINE_AUTOMATION_PREFIX.as_bytes()),
        (cf::CF_KV, taint_prefix.as_slice()),
    ] {
        let rows = db.scan_cf_prefix(cf_name, prefix).map_err(read_error)?;
        ids.extend(
            rows.iter()
                .filter_map(|(key, _value)| key_suffix(key, prefix)),
        );
    }
    Ok(ids)
}

/// The UTF-8 id after `prefix` in `key`, when non-empty.
fn key_suffix(key: &[u8], prefix: &[u8]) -> Option<String> {
    let suffix = key.strip_prefix(prefix)?;
    let id = std::str::from_utf8(suffix).ok()?;
    (!id.is_empty()).then(|| id.to_owned())
}
