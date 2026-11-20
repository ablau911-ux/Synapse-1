//! Retained schedules, old ledger compatibility, and migration bypass guards.
use super::*;

fn schedule_source(db: &Db, morning: Option<u32>, afternoon: Option<u32>) -> RoutineMineParams {
    let params = replace_episodes(db, morning, afternoon);
    rename_episode_app(db, "outlook", "outlook.exe");
    params
}

fn schedules(db: &Db) -> BTreeMap<u32, RoutineRecord> {
    stored_routines(db)
        .into_iter()
        .filter(|row| row.steps.len() == 3)
        .map(|row| (row.mean_minute_of_day, row))
        .collect()
}

fn strip_schedule(db: &Db) {
    let rows = db
        .scan_cf_prefix(cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes())
        .unwrap()
        .into_iter()
        .map(|(key, bytes)| {
            let mut value: serde_json::Value = synapse_storage::decode_json(&bytes).unwrap();
            value.as_object_mut().unwrap().remove("schedule");
            (key, encode_json(&value).unwrap())
        });
    db.put_batch_pressure_bypass(cf::CF_KV, rows).unwrap();
}

#[test]
fn must_pass_same_raw_steps_return_to_their_own_schedules_after_empty_pass_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let params = schedule_source(&db, Some(480), Some(900));
    mine_and_store_routines(&db, &params).unwrap();
    let before = schedules(&db);
    assert_eq!(before.len(), 2);
    let (morning, morning_refs) = seed_owner_rows(&db, &before[&480].routine_id, true);
    let (afternoon, afternoon_refs) = seed_owner_rows(&db, &before[&900].routine_id, false);
    mine_source(&db, None, None);
    let params = schedule_source(&db, Some(481), Some(899));
    mine_and_store_routines(&db, &params).unwrap();
    let after = schedules(&db);
    assert_eq!(after.len(), 2);
    for (minute, state, refs) in [
        (481, morning, morning_refs),
        (899, afternoon, afternoon_refs),
    ] {
        assert_eq!(
            after[&minute].routine_id, state.routine_id,
            "same raw schedules swapped"
        );
        assert_operator_payload(
            &load_state_row(&db, &state.routine_id).unwrap().unwrap(),
            &state,
        );
        assert_eq!(references_with_taint(&db, &state.routine_id), refs);
    }
}

#[test]
fn must_fail_distant_return_cannot_reuse_nearby_or_matching_hash_owner_165() {
    for same_hash in [false, true] {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
        mine_source(&db, Some(525), None);
        let mut owner = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
        let (state, refs) = seed_owner_rows(&db, &owner.routine_id, true);
        if same_hash {
            // Model an owner whose stable id survived gradual drift to 15:00.
            // Returning to its original hash anchor must not bypass tolerance.
            owner.mean_minute_of_day = 900;
            put(
                &db,
                cf::CF_KV,
                ledger_key(&owner.routine_id),
                &RoutineIdentityOwner::of(&owner),
            );
        }
        mine_source(&db, None, None);
        mine_source(&db, Some(if same_hash { 525 } else { 900 }), None);
        let distant = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
        assert_ne!(
            distant.routine_id, owner.routine_id,
            "raw steps alone cannot steal another schedule"
        );
        assert_candidate(&db, &distant.routine_id);
        assert_operator_payload(
            &load_state_row(&db, &owner.routine_id).unwrap().unwrap(),
            &state,
        );
        assert_eq!(references_with_taint(&db, &owner.routine_id), refs);
    }
}

#[test]
fn must_pass_retained_circular_schedule_matches_only_within_tolerance_165() {
    for (minute, matches) in [(5, true), (6, false)] {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
        mine_source(&db, Some(525), None);
        let mut owner = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
        let (saved, refs) = seed_owner_rows(&db, &owner.routine_id, true);
        mine_source(&db, None, None);
        // A durable id is independent of its latest minute. Plant the exact
        // circular boundary as historical metadata, then drive the real miner.
        owner.mean_minute_of_day = 1435;
        owner.tolerance_minutes = 10;
        put(
            &db,
            cf::CF_KV,
            ledger_key(&owner.routine_id),
            &RoutineIdentityOwner::of(&owner),
        );
        mine_source(&db, Some(minute), None);
        let fresh = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
        assert_eq!(
            fresh.routine_id == owner.routine_id,
            matches,
            "retained circular schedule boundary was ignored"
        );
        assert_operator_payload(
            &load_state_row(&db, &owner.routine_id).unwrap().unwrap(),
            &saved,
        );
        assert_eq!(references_with_taint(&db, &owner.routine_id), refs);
        if !matches {
            assert_candidate(&db, &fresh.routine_id);
        }
    }
}

#[test]
fn must_pass_v1_owner_without_schedule_recovers_when_raw_identity_is_unambiguous_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let owners = pair_fixture(&db);
    mine_source(&db, None, None);
    strip_schedule(&db);
    mine_source(&db, Some(526), None);
    assert_owners(&db, &owners, &["outlook.exe"]);
}

#[test]
fn must_pass_v1_schedules_are_backfilled_before_current_records_disappear_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let params = schedule_source(&db, Some(480), Some(900));
    mine_and_store_routines(&db, &params).unwrap();
    let before = schedules(&db);
    assert_eq!(before.len(), 2);
    strip_schedule(&db);
    mine_source(&db, None, None);
    let params = schedule_source(&db, None, Some(899));
    mine_and_store_routines(&db, &params).unwrap();
    let after = schedules(&db);
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[&899].routine_id, before[&900].routine_id,
        "empty mine discarded recoverable v1 schedule metadata"
    );
}

#[test]
fn must_fail_scheduleless_ambiguous_owners_remain_reserved_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let params = schedule_source(&db, Some(480), Some(900));
    mine_and_store_routines(&db, &params).unwrap();
    let before = schedules(&db);
    assert_eq!(before.len(), 2);
    mine_source(&db, None, None);
    strip_schedule(&db);
    let params = schedule_source(&db, Some(481), None);
    mine_and_store_routines(&db, &params).unwrap();
    let after = schedules(&db);
    assert_eq!(after.len(), 1);
    for owner in before.values() {
        assert_ne!(
            after[&481].routine_id, owner.routine_id,
            "missing schedule led to guessed identity"
        );
    }
    assert_candidate(&db, &after[&481].routine_id);
}

#[test]
fn must_fail_legacy_migration_cannot_bypass_retained_owner_precedence_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let owners = pair_fixture(&db);
    mine_source(&db, Some(525), None);
    clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
    let previous_state = db.scan_cf_prefix(cf::CF_ROUTINE_STATE, b"").unwrap();
    let summary = migrate_legacy_routine_ids(
        &db,
        std::slice::from_ref(&owners.records["outlook"]),
        &mut 0,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        summary.routines_remapped, 0,
        "migration signature fallback stole returning owner's id"
    );
    assert_eq!(summary.state_rows_moved, 0);
    assert_eq!(summary.armed_rows_moved, 0);
    assert_eq!(summary.automation_rows_moved, 0);
    assert_eq!(
        db.scan_cf_prefix(cf::CF_ROUTINE_STATE, b"").unwrap(),
        previous_state
    );
    assert_eq!(
        references_with_taint(&db, &owners.records["outlook"].routine_id),
        owners.references["outlook"]
    );
}
