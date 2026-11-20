//! One-time remap of legacy unstable routine ids onto content-derived stable
//! ids (#165).
//!
//! Before #165 a routine id hashed the day-of-week class and a global time
//! ordinal, so ids shifted between mining runs and operator state
//! (`CF_ROUTINE_STATE`), armed automations (`armed_routine/v1`), and
//! automation rows (`routine_automation/v1`) could attach to the wrong
//! routine. The first non-dry-run mining pass after the upgrade runs this
//! migration BEFORE `CF_ROUTINES` is replaced:
//!
//! - every previously stored routine is paired with the freshly mined routine
//!   that has the same normalized step signature
//!   ([`synapse_core::routine_identity::match_routines_by_signature`]);
//! - state, armed, and automation rows keyed by the old id move to the new id
//!   (content untouched);
//! - an armed row that cannot be matched is KEPT but disarmed with a logged
//!   reason — it is never silently re-bound to a guessed routine;
//! - a marker row in `CF_KV` makes the migration run exactly once. Every step
//!   tolerates already-moved rows, so a crash between steps is repaired by
//!   the next pass.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rmcp::ErrorData;
use serde::{Deserialize, Serialize};
use synapse_core::error_codes;
use synapse_core::routine_identity::match_routines_by_signature_retained;
use synapse_core::types::RoutineRecord;
use synapse_storage::{Db, cf, decode_json, encode_json, routines as routine_codec};

use crate::m1::mcp_error;

use super::armed_routines::{ArmedRoutineRecord, armed_routine_key, load_all_armed_routines};
use super::episodes::now_ts_ns;
use super::profile_authoring::{
    ROUTINE_AUTOMATION_PREFIX, RoutineAutomationRecord, routine_automation_key,
};
use super::routine_identity_ledger::{RetainedIdentityRows, validate_identity_records};
use super::routines::{load_all_routine_records, load_all_state_rows};

/// Planned CF_KV row deletes and puts, applied together as one batch.
type RowWrites<'a> = (&'a mut Vec<Vec<u8>>, &'a mut Vec<(Vec<u8>, Vec<u8>)>);

/// `CF_KV` key of the once-only marker row.
pub const ROUTINE_ID_MIGRATION_MARKER_KEY: &str = "routine_id_migration/v1/stable_ids";
/// Actor recorded on rows this migration disarms.
pub const ROUTINE_ID_MIGRATION_ACTOR: &str = "miner:routine_id_migration";

/// Outcome of one migration pass; also the persisted marker value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyIdMigration {
    pub migrated_ts_ns: u64,
    pub routines_remapped: u64,
    pub state_rows_moved: u64,
    pub armed_rows_moved: u64,
    pub armed_rows_disarmed: u64,
    pub automation_rows_moved: u64,
}

fn read_error(error: impl std::fmt::Display) -> ErrorData {
    mcp_error(
        error_codes::STORAGE_READ_FAILED,
        format!("ROUTINE_ID_MIGRATION_READ_FAILED: {error}"),
    )
}

fn write_error(error: synapse_storage::StorageError) -> ErrorData {
    mcp_error(
        error.code(),
        format!("ROUTINE_ID_MIGRATION_WRITE_FAILED: {error}"),
    )
}

/// Runs the legacy-id migration once against the ownership evidence stored
/// right now. Returns `Ok(None)` when the marker row says it already ran.
/// `mined` is the routine set about to replace `CF_ROUTINES`; the stored
/// (old) routines are read before that happens.
#[cfg(test)]
pub(super) fn migrate_legacy_routine_ids(
    db: &Arc<Db>,
    mined: &[RoutineRecord],
    scanned_rows: &mut u64,
) -> Result<Option<LegacyIdMigration>, ErrorData> {
    migrate_with_snapshot(db, mined, None, scanned_rows)
}

/// Mining entry point. `retained` is the ownership snapshot the pass
/// reconciled against, read before the ledger write. Mining admits the ledger
/// batch first (its only pressure/capacity-checked write); owners recorded by
/// that batch must not change this migration's remap decisions.
pub(super) fn migrate_legacy_routine_ids_retained(
    db: &Arc<Db>,
    mined: &[RoutineRecord],
    retained: &RetainedIdentityRows,
    scanned_rows: &mut u64,
) -> Result<Option<LegacyIdMigration>, ErrorData> {
    migrate_with_snapshot(db, mined, Some(retained), scanned_rows)
}

fn migrate_with_snapshot(
    db: &Arc<Db>,
    mined: &[RoutineRecord],
    snapshot: Option<&RetainedIdentityRows>,
    scanned_rows: &mut u64,
) -> Result<Option<LegacyIdMigration>, ErrorData> {
    let marker = db
        .get_cf_exact(cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes())
        .map_err(read_error)?;
    if marker.is_some() {
        return Ok(None);
    }
    let old: Vec<RoutineRecord> = load_all_routine_records(db, scanned_rows)?
        .into_values()
        .collect();
    validate_identity_records(&old)?;
    validate_identity_records(mined)?;
    let loaded;
    let retained = match snapshot {
        Some(retained) => retained,
        None => {
            loaded = RetainedIdentityRows::load(db)?;
            &loaded
        }
    };
    let remap: BTreeMap<String, String> =
        match_routines_by_signature_retained(&old, retained.retained(), mined)
            .into_iter()
            .collect();
    apply_legacy_remap(db, mined, &remap, scanned_rows).map(Some)
}

fn apply_legacy_remap(
    db: &Arc<Db>,
    mined: &[RoutineRecord],
    remap: &BTreeMap<String, String>,
    scanned_rows: &mut u64,
) -> Result<LegacyIdMigration, ErrorData> {
    let new_ids: BTreeSet<&str> = mined
        .iter()
        .map(|routine| routine.routine_id.as_str())
        .collect();
    let now = now_ts_ns();
    let mut summary = LegacyIdMigration {
        migrated_ts_ns: now,
        routines_remapped: u64::try_from(remap.len()).unwrap_or(u64::MAX),
        ..LegacyIdMigration::default()
    };

    let blocked = move_state_rows(db, remap, scanned_rows, &mut summary)?;
    persist_legacy_reference_changes(db, remap, &blocked, &new_ids, &mut summary)?;
    log_migration_summary(&summary);
    Ok(summary)
}

fn persist_legacy_reference_changes(
    db: &Arc<Db>,
    remap: &BTreeMap<String, String>,
    blocked: &BTreeSet<String>,
    new_ids: &BTreeSet<&str>,
    summary: &mut LegacyIdMigration,
) -> Result<(), ErrorData> {
    let now = summary.migrated_ts_ns;
    let mut deletes: Vec<Vec<u8>> = Vec::new();
    let mut puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    plan_armed_changes(
        db,
        remap,
        blocked,
        new_ids,
        now,
        summary,
        (&mut deletes, &mut puts),
    )?;
    plan_automation_moves(db, remap, blocked, now, summary, (&mut deletes, &mut puts))?;
    puts.push((
        ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes().to_vec(),
        encode_json(&summary).map_err(write_error)?,
    ));
    db.mutate_batch_pressure_bypass(cf::CF_KV, deletes, puts)
        .map_err(write_error)
}

fn log_migration_summary(summary: &LegacyIdMigration) {
    tracing::info!(
        code = "ROUTINE_ID_MIGRATION_DONE",
        routines_remapped = summary.routines_remapped,
        state_rows_moved = summary.state_rows_moved,
        armed_rows_moved = summary.armed_rows_moved,
        armed_rows_disarmed = summary.armed_rows_disarmed,
        automation_rows_moved = summary.automation_rows_moved,
        "remapped legacy routine ids onto stable content-derived ids"
    );
}

/// Moves `CF_ROUTINE_STATE` rows to their new ids. Returns the old ids whose
/// move was blocked by an existing row at the target (kept untouched).
fn move_state_rows(
    db: &Arc<Db>,
    remap: &BTreeMap<String, String>,
    scanned_rows: &mut u64,
    summary: &mut LegacyIdMigration,
) -> Result<BTreeSet<String>, ErrorData> {
    let states = load_all_state_rows(db, scanned_rows)?;
    let mut blocked = BTreeSet::new();
    let mut deletes: Vec<Vec<u8>> = Vec::new();
    let mut puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (old_id, new_id) in remap {
        let Some(state) = states.get(old_id) else {
            continue;
        };
        if states.contains_key(new_id) {
            tracing::warn!(
                code = "ROUTINE_ID_MIGRATION_STATE_TARGET_EXISTS",
                old_id = %old_id,
                new_id = %new_id,
                "state row already exists at the stable id; old state row kept, not merged"
            );
            blocked.insert(old_id.clone());
            continue;
        }
        let mut moved = state.clone();
        moved.routine_id.clone_from(new_id);
        deletes.push(routine_codec::routine_state_key(old_id).map_err(write_error)?);
        puts.push((
            routine_codec::routine_state_key(new_id).map_err(write_error)?,
            encode_json(&moved).map_err(write_error)?,
        ));
        summary.state_rows_moved += 1;
    }
    db.mutate_batch_pressure_bypass(cf::CF_ROUTINE_STATE, deletes, puts)
        .map_err(write_error)?;
    Ok(blocked)
}

/// Moves matched armed rows to the new id and disarms unmatched enabled ones.
fn plan_armed_changes(
    db: &Arc<Db>,
    remap: &BTreeMap<String, String>,
    blocked: &BTreeSet<String>,
    new_ids: &BTreeSet<&str>,
    now: u64,
    summary: &mut LegacyIdMigration,
    (deletes, puts): RowWrites<'_>,
) -> Result<(), ErrorData> {
    let armed = load_all_armed_routines(db)?;
    let armed_ids: BTreeSet<String> = armed
        .iter()
        .map(|record| record.routine_id.clone())
        .collect();
    for mut record in armed {
        let old_id = record.routine_id.clone();
        if new_ids.contains(old_id.as_str()) {
            continue;
        }
        let target = remap
            .get(&old_id)
            .filter(|new_id| !blocked.contains(&old_id) && !armed_ids.contains(new_id.as_str()));
        if let Some(new_id) = target {
            deletes.push(armed_routine_key(&old_id).into_bytes());
            record.routine_id.clone_from(new_id);
            record.updated_at_ns = now;
            puts.push((
                armed_routine_key(new_id).into_bytes(),
                encode_json(&record).map_err(write_error)?,
            ));
            summary.armed_rows_moved += 1;
        } else if record.enabled {
            disarm_unmatched_legacy(&mut record, now);
            puts.push((
                armed_routine_key(&old_id).into_bytes(),
                encode_json(&record).map_err(write_error)?,
            ));
            summary.armed_rows_disarmed += 1;
        }
    }
    Ok(())
}

fn disarm_unmatched_legacy(record: &mut ArmedRoutineRecord, now: u64) {
    let old_id = &record.routine_id;
    let reason = format!(
        "ROUTINE_ID_UNMATCHED: armed routine {old_id} has no re-mined routine with the \
         same step signature after the stable-id migration (#165); disarmed instead of \
         re-binding to a guessed routine"
    );
    tracing::warn!(
        code = "ROUTINE_ID_MIGRATION_ARMED_DISARMED",
        routine_id = %old_id,
        reason = %reason,
        "disarming armed routine that cannot be matched to a stable id"
    );
    record.enabled = false;
    record.updated_at_ns = now;
    record.disarmed_at_ns = Some(now);
    record.disarmed_by = Some(ROUTINE_ID_MIGRATION_ACTOR.to_owned());
    record.disarm_reason = Some(reason);
}

/// Moves matched `routine_automation/v1` rows to the new id. Unmatched rows
/// stay as they are: an automation row alone never runs anything.
fn plan_automation_moves(
    db: &Arc<Db>,
    remap: &BTreeMap<String, String>,
    blocked: &BTreeSet<String>,
    now: u64,
    summary: &mut LegacyIdMigration,
    (deletes, puts): RowWrites<'_>,
) -> Result<(), ErrorData> {
    let rows = db
        .scan_cf_prefix(cf::CF_KV, ROUTINE_AUTOMATION_PREFIX.as_bytes())
        .map_err(read_error)?;
    let existing: BTreeSet<Vec<u8>> = rows.iter().map(|(key, _value)| key.clone()).collect();
    for (key, value) in &rows {
        let mut record = decode_json::<RoutineAutomationRecord>(value).map_err(|error| {
            mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!("ROUTINE_AUTOMATION_ROW_DECODE_FAILED during id migration: {error}"),
            )
        })?;
        let old_id = record.routine_id.clone();
        let Some(new_id) = remap.get(&old_id).filter(|_| !blocked.contains(&old_id)) else {
            continue;
        };
        let new_key = routine_automation_key(new_id).into_bytes();
        if existing.contains(&new_key) {
            tracing::warn!(
                code = "ROUTINE_ID_MIGRATION_AUTOMATION_TARGET_EXISTS",
                old_id = %old_id,
                new_id = %new_id,
                "automation row already exists at the stable id; old row kept"
            );
            continue;
        }
        deletes.push(key.clone());
        record.routine_id.clone_from(new_id);
        record.updated_at_ns = now;
        puts.push((new_key, encode_json(&record).map_err(write_error)?));
        summary.automation_rows_moved += 1;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/routine_id_migration_165.rs"]
mod routine_id_migration_165_tests;
