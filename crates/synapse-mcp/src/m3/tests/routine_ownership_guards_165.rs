//! Defensive writer and migration guards supplement the persisted scenarios.
use super::*;
use crate::m3::routine_identity_ledger::RetainedIdentityRows;

#[test]
fn must_fail_ledger_writer_refuses_reassignment_without_partial_writes_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let owners = pair_fixture(&db);
    let retained = RetainedIdentityRows::load(&db).expect("real ledger load");
    let mut wrong = owners.records["outlook"].clone();
    wrong.steps = owners.records["outlook.exe"].steps.clone();
    let before = snapshot(&db);
    let error = retained
        .persist(&db, &[], &[wrong])
        .expect_err("immutable raw owner");
    assert_eq!(
        error.data.as_ref().unwrap()["code"],
        error_codes::STORAGE_CORRUPTED
    );
    assert_eq!(snapshot(&db), before, "rejected owner write must be atomic");
}

#[test]
fn must_fail_migration_cannot_rename_an_already_recorded_owner_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    mine_source(&db, Some(525), None);
    let mut fresh = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
    let original = fresh.routine_id.clone();
    let (state, _) = seed_owner_rows(&db, &original, true);
    fresh.routine_id = LEGACY_EXCEL.into();
    clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
    let summary = migrate_legacy_routine_ids(&db, &[fresh], &mut 0)
        .unwrap()
        .unwrap();
    assert_eq!(
        summary.routines_remapped, 0,
        "recorded identity cannot be renamed by migration"
    );
    assert_eq!(load_state_row(&db, &original).unwrap(), Some(state));
    assert!(load_state_row(&db, LEGACY_EXCEL).unwrap().is_none());
}

#[test]
fn must_fail_migration_cannot_target_another_retained_owner_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    mine_source(&db, Some(525), None);
    let mut candidate = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
    let original = candidate.routine_id.clone();
    let (state, _) = seed_owner_rows(&db, &original, true);
    forget_ledger(&db);
    candidate.routine_id = LEGACY_EXCEL.into();
    let mut foreign = candidate.clone();
    foreign.steps[0].app = "word.exe".into();
    foreign.mean_minute_of_day = 900;
    put(
        &db,
        cf::CF_KV,
        ledger_key(LEGACY_EXCEL),
        &RoutineIdentityOwner::of(&foreign),
    );
    clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
    let summary = migrate_legacy_routine_ids(&db, &[candidate], &mut 0)
        .unwrap()
        .unwrap();
    assert_eq!(
        summary.routines_remapped, 0,
        "migration overwrote a retained destination"
    );
    assert_eq!(load_state_row(&db, &original).unwrap(), Some(state));
    assert!(load_state_row(&db, LEGACY_EXCEL).unwrap().is_none());
}

#[test]
fn must_fail_corrupt_state_or_armed_rows_refuse_before_ownership_writes_165() {
    for armed_row in [false, true] {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
        let owners = pair_fixture(&db);
        forget_ledger(&db);
        let id = &owners.records["outlook.exe"].routine_id;
        let (cf_name, key) = if armed_row {
            (cf::CF_KV, armed_routine_key(id).into_bytes())
        } else {
            (
                cf::CF_ROUTINE_STATE,
                routine_codec::routine_state_key(id).unwrap(),
            )
        };
        db.put_batch_pressure_bypass(cf_name, [(key, b"invalid JSON".to_vec())])
            .unwrap();
        let params = replace_episodes(&db, None, None);
        let before = snapshot(&db);
        mine_and_store_routines(&db, &params)
            .expect_err("dependent corruption must fail before writes");
        assert_eq!(snapshot(&db), before);
    }
}

#[test]
fn must_fail_invalid_preledger_records_cannot_poison_ownership_or_mutate_rows_165() {
    for empty in [false, true] {
        for invalid_schedule in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
            mine_source(&db, Some(525), None);
            let mut old = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
            seed_owner_rows(&db, &old.routine_id, true);
            if invalid_schedule {
                old.mean_minute_of_day = u32::MAX;
            } else {
                old.steps.clear();
            }
            rewrite_current(&db, &old);
            forget_ledger(&db);
            clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
            let params = replace_episodes(&db, (!empty).then_some(525), None);
            let before = snapshot(&db);
            let error =
                mine_and_store_routines(&db, &params).expect_err("invalid bootstrap evidence");
            assert_eq!(
                error.data.as_ref().unwrap()["code"],
                error_codes::STORAGE_CORRUPTED
            );
            assert_eq!(
                snapshot(&db),
                before,
                "invalid current evidence changed ownership/state"
            );
        }
    }
}
