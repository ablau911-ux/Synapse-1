//! Upgrade evidence, immutable owners, malformed ledgers and legacy fallback.
#[path = "routine_ownership_documents_165.rs"]
mod routine_ownership_documents_165;
#[path = "routine_ownership_guards_165.rs"]
mod routine_ownership_guards_165;
#[path = "routine_ownership_schedule_165.rs"]
mod routine_ownership_schedule_165;

use super::*;

fn forget_ledger(db: &Db) {
    clear_cf(db, cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes());
}

fn rewrite_current(db: &Db, old: &RoutineRecord) {
    clear_cf(db, cf::CF_ROUTINES, b"");
    put(
        db,
        cf::CF_ROUTINES,
        routine_codec::routine_key(&old.routine_id).expect("key"),
        old,
    );
}

fn rename_episode_app(db: &Db, from: &str, to: &str) {
    let rows = db
        .scan_cf_prefix(cf::CF_EPISODES, b"")
        .expect("source rows")
        .into_iter()
        .filter_map(|(key, value)| {
            let mut row: synapse_core::types::EpisodeRecord =
                synapse_storage::decode_json(&value).expect("source decode");
            if row.app.as_deref() != Some(from) {
                return None;
            }
            row.app = Some(to.to_owned());
            row.episode_id = synapse_core::episodes::episode_id(
                row.start_ts_ns,
                &row.actor,
                row.app.as_deref(),
                row.document.as_deref(),
            );
            Some((key, encode_json(&row).expect("source encode")))
        });
    db.put_batch_pressure_bypass(cf::CF_EPISODES, rows)
        .expect("rename source app");
}

#[test]
fn must_pass_preledger_current_owners_survive_first_empty_upgrade_mine_165() {
    for migration_already_ran in [true, false] {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
        let mut owners = pair_fixture(&db);
        forget_ledger(&db);
        if !migration_already_ran {
            clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
        }
        let params = replace_episodes(&db, None, None);
        preview_then_commit(&db, params);
        if !migration_already_ran {
            // Existing one-time migration intentionally disarms absent owners.
            for (app, row) in &owners.records {
                let armed = load_armed_routine_record(&db, &row.routine_id)
                    .expect("armed read")
                    .expect("armed owner retained");
                assert!(!armed.enabled);
                owners
                    .references
                    .insert(app.clone(), references_with_taint(&db, &row.routine_id));
            }
        }
        assert_owners(&db, &owners, &[]);
        mine_source(&db, Some(526), None);
        assert_owners(&db, &owners, &["outlook.exe"]);
    }
}

#[test]
fn must_pass_unambiguous_preledger_cosmetic_alias_is_bootstrapped_once_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    mine_source(&db, Some(525), None);
    let original = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
    let (state, refs) = seed_owner_rows(&db, &original.routine_id, true);
    let mut legacy = original.clone();
    legacy.steps[0].app = " Outlook ".into();
    rewrite_current(&db, &legacy);
    forget_ledger(&db);
    mine_source(&db, Some(526), None);
    let current = by_spelling(&stored_routines(&db));
    assert_eq!(current["outlook.exe"].routine_id, original.routine_id);
    assert_operator_payload(
        &load_state_row(&db, &original.routine_id).unwrap().unwrap(),
        &state,
    );
    assert_eq!(references_with_taint(&db, &original.routine_id), refs);
    // Once recorded, another raw spelling is a newcomer, even in isolation.
    mine_source(&db, None, Some(526));
    let newcomer = by_spelling(&stored_routines(&db))["outlook"].clone();
    assert_ne!(
        newcomer.routine_id, original.routine_id,
        "post-ledger alias stole ownership"
    );
    assert_candidate(&db, &newcomer.routine_id);
    assert_eq!(references_with_taint(&db, &original.routine_id), refs);
}

#[test]
fn must_fail_ambiguous_preledger_signature_cannot_choose_a_sibling_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    mine_source(&db, Some(525), None);
    let mut legacy = by_spelling(&stored_routines(&db))["outlook.exe"].clone();
    let (state, refs) = seed_owner_rows(&db, &legacy.routine_id, true);
    legacy.steps[0].app = " outlook.exe".into();
    rewrite_current(&db, &legacy);
    forget_ledger(&db);
    mine_source(&db, Some(525), Some(525));
    let rows = by_spelling(&stored_routines(&db));
    assert_eq!(rows.len(), 2);
    for row in rows.values() {
        assert_ne!(
            row.routine_id, legacy.routine_id,
            "ambiguous signature transferred ownership"
        );
        assert_candidate(&db, &row.routine_id);
    }
    assert_operator_payload(
        &load_state_row(&db, &legacy.routine_id).unwrap().unwrap(),
        &state,
    );
    assert_eq!(references_with_taint(&db, &legacy.routine_id), refs);
}

#[test]
fn must_fail_owner_unknown_preledger_rows_cannot_be_claimed_by_fresh_ids_165() {
    for family in 0..4 {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
        mine_source(&db, Some(525), None);
        let id = by_spelling(&stored_routines(&db))["outlook.exe"]
            .routine_id
            .clone();
        let (state, refs) = seed_owner_rows(&db, &id, true);
        clear_cf(&db, cf::CF_ROUTINES, b"");
        clear_cf(&db, cf::CF_ROUTINE_STATE, b"");
        for (key, _) in &refs {
            clear_cf(&db, cf::CF_KV, key);
        }
        forget_ledger(&db);
        if family == 0 {
            put(
                &db,
                cf::CF_ROUTINE_STATE,
                routine_codec::routine_state_key(&id).unwrap(),
                &state,
            );
        } else {
            let (key, bytes) = &refs[family - 1];
            db.put_batch_pressure_bypass(cf::CF_KV, [(key.clone(), bytes.clone().unwrap())])
                .expect("seed only one unknown-owner family");
        }
        mine_source(&db, Some(525), None);
        let fresh = by_spelling(&stored_routines(&db))["outlook.exe"]
            .routine_id
            .clone();
        assert_ne!(
            fresh, id,
            "unknown owner reused via dependent family {family}"
        );
        assert_candidate(&db, &fresh);
        if family == 0 {
            assert_operator_payload(&load_state_row(&db, &id).unwrap().unwrap(), &state);
        } else {
            let (key, bytes) = &refs[family - 1];
            assert_eq!(db.get_cf_exact(cf::CF_KV, key).unwrap(), *bytes);
        }
    }
}

#[test]
fn must_fail_current_record_cannot_override_recorded_raw_owner_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let owners = pair_fixture(&db);
    let mut wrong = owners.records["outlook.exe"].clone();
    wrong.routine_id = owners.records["outlook"].routine_id.clone();
    rewrite_current(&db, &wrong);
    mine_source(&db, Some(525), None);
    assert_owners(&db, &owners, &["outlook.exe"]);
    mine_source(&db, None, Some(525));
    assert_owners(&db, &owners, &["outlook"]);
}

#[test]
fn must_fail_malformed_ledger_refuses_mining_without_any_writes_165() {
    for empty in [false, true] {
        for corruption in 0..3 {
            let dir = tempfile::tempdir().expect("tempdir");
            let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
            let owners = pair_fixture(&db);
            let id = &owners.records["outlook.exe"].routine_id;
            let mut value =
                serde_json::to_value(RoutineIdentityOwner::of(&owners.records["outlook.exe"]))
                    .expect("owner JSON");
            let bytes = match corruption {
                0 => b"broken JSON".to_vec(),
                1 => {
                    value["routine_id"] = serde_json::json!(LEGACY_EXCEL);
                    encode_json(&value).unwrap()
                }
                _ => {
                    value["schedule"] =
                        serde_json::json!({"mean_minute_of_day": 1440, "tolerance_minutes": 10});
                    encode_json(&value).unwrap()
                }
            };
            db.put_batch_pressure_bypass(cf::CF_KV, [(ledger_key(id), bytes)])
                .unwrap();
            let params = replace_episodes(&db, (!empty).then_some(525), None);
            let before = snapshot(&db);
            let error = mine_and_store_routines(&db, &params)
                .expect_err("corrupt ownership must fail closed");
            assert_eq!(
                error.data.as_ref().unwrap()["code"],
                error_codes::STORAGE_CORRUPTED
            );
            assert_eq!(
                snapshot(&db),
                before,
                "corrupt ledger caused partial migration or deletion"
            );
        }
    }
}
