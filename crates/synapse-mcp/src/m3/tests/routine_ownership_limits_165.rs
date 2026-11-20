//! #165 owner follow-up: bounded plaintext ledger; pressure-aware atomic refusal.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "routine_mine_admission_165.rs"]
mod routine_mine_admission_165;

use super::*;
use crate::m3::routine_identity_ledger::RetainedIdentityRows;
use synapse_storage::DiskPressureLevel;

// Contract selected for the fixer: exact logical key + encoded-value bytes,
// namespace only. Never evict an owner, including ledger-only reservations.
// Independent literal deliberately avoids compiling against a missing fix API.
const LEDGER_BYTE_CAP: usize = 1024 * 1024;
const PADDING_ID: &str = "rt1-fffffffffffffffe";

fn ledger_bytes(db: &Db) -> usize {
    db.scan_cf_prefix(cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes())
        .expect("ledger rows")
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum()
}

fn owner_bytes(record: &RoutineRecord) -> usize {
    ledger_key(&record.routine_id).len()
        + encode_json(&RoutineIdentityOwner::of(record))
            .expect("owner encoding")
            .len()
}

fn pad_ledger_to(db: &Db, template: &RoutineRecord, target: usize) {
    let mut padding = RoutineIdentityOwner::of(template);
    padding.routine_id = PADDING_ID.into();
    padding.steps[0].app = "reserved-padding-".into();
    let key = ledger_key(PADDING_ID);
    assert!(db.get_cf_exact(cf::CF_KV, &key).unwrap().is_none());
    let base = ledger_bytes(db) + key.len() + encode_json(&padding).unwrap().len();
    let remaining = target.checked_sub(base).expect("padding budget");
    padding.steps[0].app.push_str(&"x".repeat(remaining));
    put(db, cf::CF_KV, key, &padding);
    assert_eq!(ledger_bytes(db), target, "exact physical byte boundary");
}

fn new_owner(template: &RoutineRecord, suffix: u64) -> RoutineRecord {
    let mut row = template.clone();
    row.routine_id = format!("rt1-{suffix:016x}");
    row.steps[0].app = format!("new-owner-{suffix}.exe");
    row
}

fn assert_cap_error(error: &rmcp::ErrorData) {
    assert_eq!(
        error.data.as_ref().expect("typed capacity refusal")["code"],
        error_codes::STORAGE_CF_HARD_CAP_REACHED
    );
    assert!(
        error.message.contains("routine_identity"),
        "capacity error must identify the bounded namespace: {error}"
    );
}

fn assert_pressure_error(error: &rmcp::ErrorData) {
    assert_eq!(
        error.data.as_ref().expect("typed pressure refusal")["code"],
        error_codes::STORAGE_WRITE_SHED,
        "preserve storage pressure code through the ledger writer"
    );
}

#[test]
fn must_fail_ledger_at_byte_cap_refuses_mine_without_partial_writes_165() {
    for missing_marker in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
        let owners = pair_fixture(&db);
        pad_ledger_to(&db, &owners.records["outlook.exe"], LEDGER_BYTE_CAP);
        if missing_marker {
            clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
        }
        let params = replace_spellings(&db, Some(" outlook.exe"));
        let before = snapshot(&db);
        let error = mine_and_store_routines(&db, &params)
            .expect_err("#165: full ledger must refuse a new identity, never evict");
        assert_cap_error(&error);
        assert_eq!(
            snapshot(&db),
            before,
            "no migration, ledger or dependent writes"
        );
        assert_eq!(ledger_bytes(&db), LEDGER_BYTE_CAP);
    }
}

#[test]
fn must_fail_empty_upgrade_at_byte_cap_refuses_owner_backfill_165() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
    let owners = pair_fixture(&db);
    clear_cf(&db, cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes());
    pad_ledger_to(&db, &owners.records["outlook.exe"], LEDGER_BYTE_CAP);
    clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
    let params = replace_episodes(&db, None, None);
    let before = snapshot(&db);
    let error = mine_and_store_routines(&db, &params)
        .expect_err("#165: previous owners also consume capacity on an empty upgrade mine");
    assert_cap_error(&error);
    assert_eq!(
        snapshot(&db),
        before,
        "failed backfill must not delete old owners"
    );
}

#[test]
fn must_fail_ledger_batch_overflow_is_atomic_165() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
    let owners = pair_fixture(&db);
    let original = &owners.records["outlook.exe"];
    let first = new_owner(original, 1);
    let second = new_owner(original, 2);
    pad_ledger_to(&db, original, LEDGER_BYTE_CAP - owner_bytes(&first));
    let mut updated = original.clone();
    updated.mean_minute_of_day += 1;
    let retained = RetainedIdentityRows::load(&db).unwrap();
    let before = snapshot(&db);
    let error = retained
        .persist(&db, &[], &[first, second, updated])
        .expect_err("#165: preflight the whole batch, not each new owner separately");
    assert_cap_error(&error);
    assert_eq!(
        snapshot(&db),
        before,
        "no first-row insert or schedule update on refusal"
    );
}

#[test]
fn must_pass_ledger_exact_fit_and_existing_owner_at_cap_165() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
    let owners = pair_fixture(&db);
    let original = &owners.records["outlook.exe"];
    let candidate = new_owner(original, 1);
    pad_ledger_to(&db, original, LEDGER_BYTE_CAP - owner_bytes(&candidate));
    // Unrelated KV bytes must not consume this namespace's quota.
    put(
        &db,
        cf::CF_KV,
        b"unrelated/limits-165".to_vec(),
        &"z".repeat(4096),
    );
    let retained = RetainedIdentityRows::load(&db).unwrap();
    let mut oversized = candidate.clone();
    oversized.steps[0].app.push('x');
    let before = snapshot(&db);
    let error = retained
        .persist(&db, &[], &[oversized])
        .expect_err("#165: one byte over cap must fail before the exact-fit retry");
    assert_cap_error(&error);
    assert_eq!(snapshot(&db), before);
    retained
        .persist(&db, &[], std::slice::from_ref(&candidate))
        .unwrap();
    assert_eq!(ledger_bytes(&db), LEDGER_BYTE_CAP, "exact fit is accepted");
    let padding_before = db.get_cf_exact(cf::CF_KV, &ledger_key(PADDING_ID)).unwrap();
    let mut updated = candidate.clone();
    updated.mean_minute_of_day += 1;
    let retained = RetainedIdentityRows::load(&db).unwrap();
    retained
        .persist(&db, &[], std::slice::from_ref(&updated))
        .unwrap();
    retained
        .persist(&db, &[], std::slice::from_ref(&updated))
        .unwrap();
    let bytes = db
        .get_cf_exact(cf::CF_KV, &ledger_key(&candidate.routine_id))
        .unwrap()
        .unwrap();
    let saved: RoutineIdentityOwner = synapse_storage::decode_json(&bytes).unwrap();
    assert_eq!(
        saved,
        RoutineIdentityOwner::of(&updated),
        "plaintext owner and schedule"
    );
    assert_eq!(
        ledger_bytes(&db),
        LEDGER_BYTE_CAP,
        "replacement counts net bytes only"
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, &ledger_key(PADDING_ID)).unwrap(),
        padding_before
    );
    let mut growing = updated.clone();
    growing.mean_minute_of_day = 1000;
    assert_eq!(owner_bytes(&growing), owner_bytes(&updated) + 1);
    let before = snapshot(&db);
    let retained = RetainedIdentityRows::load(&db).unwrap();
    let error = retained
        .persist(&db, &[], &[growing])
        .expect_err("#165: existing-owner metadata growth also consumes the byte budget");
    assert_cap_error(&error);
    assert_eq!(snapshot(&db), before);
    assert_owners(&db, &owners, &["outlook", "outlook.exe"]);
}

#[test]
fn must_fail_ledger_pressure_refuses_new_updated_and_backfilled_owners_165() {
    for backfill in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
        let owners = pair_fixture(&db);
        let mut updated = owners.records["outlook.exe"].clone();
        updated.mean_minute_of_day += 1;
        let new = new_owner(&updated, 1);
        if backfill {
            clear_cf(&db, cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes());
        }
        let retained = RetainedIdentityRows::load(&db).unwrap();
        // Pressure arrives after load/preflight: check at the actual writer.
        let report = db.run_pressure_check_with_free_bytes_sample(0).unwrap();
        assert_eq!(report.current_level, DiskPressureLevel::Level4);
        assert!(!db.pressure_permits_write(cf::CF_KV));
        let before = snapshot(&db);
        let error = if backfill {
            retained.persist(&db, &[updated, new], &[])
        } else {
            retained.persist(&db, &[], &[updated, new])
        }
        .expect_err("#165: ledger writes must not bypass active CF_KV pressure");
        assert_pressure_error(&error);
        assert_eq!(
            snapshot(&db),
            before,
            "pressure refusal leaves every row unchanged"
        );
        retained
            .persist(&db, &[], &[])
            .expect("empty batch needs no disk write");
    }
}

#[test]
fn must_pass_ledger_mining_resumes_after_pressure_clears_165() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
    let owners = pair_fixture(&db);
    let params = replace_spellings(&db, Some(" outlook.exe"));
    let retained = RetainedIdentityRows::load(&db).unwrap();
    let candidate = new_owner(&owners.records["outlook.exe"], 1);
    db.run_pressure_check_with_free_bytes_sample(0).unwrap();
    let before = snapshot(&db);
    let error = retained
        .persist(&db, &[], &[candidate])
        .expect_err("#165: pressure refusal must precede successful recovery");
    assert_pressure_error(&error);
    assert_eq!(snapshot(&db), before);
    let report = db
        .run_pressure_check_with_free_bytes_sample(u64::MAX)
        .unwrap();
    assert_eq!(report.current_level, DiskPressureLevel::Normal);
    assert_eq!(db.scan_cf_prefix(cf::CF_EPISODES, b"").unwrap().len(), 60);
    let rows = preview_then_commit(&db, params);
    assert_mining_recovered(&db, &owners, &rows);
}

fn assert_mining_recovered(db: &Db, owners: &Owners, rows: &[RoutineRecord]) {
    let by_app = by_spelling(rows);
    let newcomer = &by_app[" outlook.exe"];
    assert_candidate(db, &newcomer.routine_id);
    assert_ne!(newcomer.routine_id, owners.records["outlook"].routine_id);
    assert_ne!(
        newcomer.routine_id,
        owners.records["outlook.exe"].routine_id
    );
    for row in rows {
        let bytes = db
            .get_cf_exact(cf::CF_KV, &ledger_key(&row.routine_id))
            .unwrap()
            .expect("mined identity persisted");
        let saved: RoutineIdentityOwner = synapse_storage::decode_json(&bytes).unwrap();
        assert_eq!(
            saved,
            RoutineIdentityOwner::of(row),
            "plaintext identity round trip"
        );
    }
    assert!(ledger_bytes(db) < LEDGER_BYTE_CAP);
    assert_eq!(
        by_app["outlook.exe"].routine_id,
        owners.records["outlook.exe"].routine_id
    );
    for (app, original) in &owners.records {
        let saved = load_state_row(db, &original.routine_id).unwrap().unwrap();
        assert_operator_payload(&saved, &owners.states[app]);
        assert_eq!(saved.present_in_last_mine, app == "outlook.exe");
        assert_eq!(
            references_with_taint(db, &original.routine_id),
            owners.references[app]
        );
        let bytes = db
            .get_cf_exact(cf::CF_KV, &ledger_key(&original.routine_id))
            .unwrap()
            .expect("old owner remains reserved");
        let owner: RoutineIdentityOwner = synapse_storage::decode_json(&bytes).unwrap();
        assert_eq!(owner.steps, original.steps);
    }
}
