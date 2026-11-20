//! Historical ownership outranks current snapshots and signature fallback.
#[path = "routine_ownership_limits_165.rs"]
mod routine_ownership_limits_165;
#[path = "routine_ownership_upgrade_165.rs"]
mod routine_ownership_upgrade_165;

use super::*;
use crate::m3::routine_identity_ledger::ROUTINE_IDENTITY_PREFIX;
use synapse_core::routine_identity::RoutineIdentityOwner;

type ReferenceRows = Vec<(Vec<u8>, Option<Vec<u8>>)>;
type StoreSnapshot = Vec<Vec<(Vec<u8>, Vec<u8>)>>;

struct Owners {
    records: BTreeMap<String, RoutineRecord>,
    states: BTreeMap<String, RoutineStateRecord>,
    references: BTreeMap<String, ReferenceRows>,
}

fn ledger_key(id: &str) -> Vec<u8> {
    format!("{ROUTINE_IDENTITY_PREFIX}{id}").into_bytes()
}

fn references_with_taint(db: &Db, id: &str) -> ReferenceRows {
    let taint = [
        crate::m3::hygiene::taint_key_prefix("routine"),
        id.as_bytes().to_vec(),
    ]
    .concat();
    [
        armed_routine_key(id).into_bytes(),
        routine_automation_key(id).into_bytes(),
        taint,
    ]
    .into_iter()
    .map(|key| {
        let bytes = db.get_cf_exact(cf::CF_KV, &key).expect("reference read");
        (key, bytes)
    })
    .collect()
}

fn seed_owner_rows(db: &Db, id: &str, exe: bool) -> (RoutineStateRecord, ReferenceRows) {
    let state = operator_state(db, id, exe);
    seed_references(db, id);
    let key = [
        crate::m3::hygiene::taint_key_prefix("routine"),
        id.as_bytes().to_vec(),
    ]
    .concat();
    put(
        db,
        cf::CF_KV,
        key,
        &serde_json::json!({"owner": id, "tainted": true}),
    );
    (state, references_with_taint(db, id))
}

fn pair_fixture(db: &Arc<Db>) -> Owners {
    let records = first_pass(db, 525, 525);
    let mut states = BTreeMap::new();
    let mut references = BTreeMap::new();
    for (app, row) in &records {
        let (state, refs) = seed_owner_rows(db, &row.routine_id, app == "outlook.exe");
        states.insert(app.clone(), state);
        references.insert(app.clone(), refs);
    }
    Owners {
        records,
        states,
        references,
    }
}

fn mine_source(db: &Arc<Db>, exe: Option<u32>, bare: Option<u32>) {
    let params = replace_episodes(db, exe, bare);
    mine_and_store_routines(db, &params).expect("real persisted mining pass");
}

fn assert_owners(db: &Db, owners: &Owners, present: &[&str]) {
    let actual = by_spelling(&stored_routines(db));
    assert_eq!(actual.len(), present.len());
    for (app, original) in &owners.records {
        let is_present = present.contains(&app.as_str());
        if is_present {
            assert_eq!(
                actual[app].routine_id, original.routine_id,
                "returning owner lost its id"
            );
        }
        let saved = load_state_row(db, &original.routine_id)
            .expect("state read")
            .expect("owner state");
        assert_operator_payload(&saved, &owners.states[app]);
        assert_eq!(saved.transitions, owners.states[app].transitions);
        assert_eq!(saved.present_in_last_mine, is_present);
        assert_eq!(
            references_with_taint(db, &original.routine_id),
            owners.references[app]
        );
        let bytes = db
            .get_cf_exact(cf::CF_KV, &ledger_key(&original.routine_id))
            .expect("ledger read")
            .expect("permanent owner");
        let owner: RoutineIdentityOwner =
            synapse_storage::decode_json(&bytes).expect("ledger decode");
        assert_eq!(owner.steps, original.steps, "ledger owner was overwritten");
        assert_eq!(owner.granularity, original.granularity);
    }
}

fn snapshot(db: &Db) -> StoreSnapshot {
    [cf::CF_ROUTINES, cf::CF_ROUTINE_STATE, cf::CF_KV]
        .iter()
        .map(|name| db.scan_cf_prefix(name, b"").expect("physical snapshot"))
        .collect()
}

fn preview_then_commit(db: &Arc<Db>, mut params: RoutineMineParams) -> Vec<RoutineRecord> {
    let before = snapshot(db);
    params.dry_run = true;
    let preview = mine_and_store_routines(db, &params).expect("preview");
    assert_eq!(
        snapshot(db),
        before,
        "preview changed stored ownership or references"
    );
    params.dry_run = false;
    mine_and_store_routines(db, &params).expect("commit");
    let persisted = stored_routines(db);
    assert_eq!(
        ids(&preview.routines),
        ids(&persisted),
        "preview/commit identity mismatch"
    );
    persisted
}

fn clear_cf(db: &Db, name: &str, prefix: &[u8]) {
    let keys = db
        .scan_cf_prefix(name, prefix)
        .expect("read deletion keys")
        .into_iter()
        .map(|(key, _)| key);
    db.mutate_batch_pressure_bypass(name, keys, Vec::<(Vec<u8>, Vec<u8>)>::new())
        .expect("clear fixture rows");
}

#[test]
fn must_fail_returning_owner_cannot_take_current_sibling_identity_165() {
    for returning in ["outlook", "outlook.exe"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
        let owners = pair_fixture(&db);
        let exe = returning == "outlook.exe";
        mine_source(&db, (!exe).then_some(525), exe.then_some(525));
        assert_owners(&db, &owners, &[if exe { "outlook" } else { "outlook.exe" }]);
        let params = replace_episodes(&db, exe.then_some(525), (!exe).then_some(525));
        preview_then_commit(&db, params);
        assert_owners(&db, &owners, &[returning]);
        mine_source(&db, Some(526), Some(524));
        assert_owners(&db, &owners, &["outlook", "outlook.exe"]);
    }
}

#[test]
fn must_pass_each_tie_owner_returns_after_empty_mine_and_database_reopen_165() {
    for returning in ["outlook", "outlook.exe"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
        let owners = pair_fixture(&db);
        mine_source(&db, None, None);
        assert_owners(&db, &owners, &[]);
        drop(db);
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("reopen persisted db"));
        let params = replace_episodes(
            &db,
            (returning == "outlook.exe").then_some(526),
            (returning == "outlook").then_some(526),
        );
        preview_then_commit(&db, params);
        assert_owners(&db, &owners, &[returning]);
    }
}

#[test]
fn must_pass_displaced_owner_returns_even_after_anchor_changes_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    mine_source(&db, Some(525), None);
    let owners = pair_fixture(&db);
    let bare = &owners.records["outlook"];
    assert_ne!(
        bare.routine_id,
        routine_id(bare.granularity, &bare.steps, 1965)
    );
    mine_source(&db, None, None);
    mine_source(&db, None, Some(526));
    assert_owners(&db, &owners, &["outlook"]);
}

#[test]
fn must_fail_ledger_only_owner_cannot_be_recycled_after_dependents_are_deleted_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let owners = pair_fixture(&db);
    mine_source(&db, None, None);
    clear_cf(&db, cf::CF_ROUTINE_STATE, b"");
    for refs in owners.references.values() {
        db.mutate_batch_pressure_bypass(
            cf::CF_KV,
            refs.iter().map(|(key, _)| key.clone()),
            Vec::<(Vec<u8>, Vec<u8>)>::new(),
        )
        .expect("delete dependent rows only");
    }
    mine_source(&db, None, None);
    let params = replace_spellings(&db, Some(" outlook.exe"));
    mine_and_store_routines(&db, &params).expect("new sibling after two empty passes");
    let rows = by_spelling(&stored_routines(&db));
    assert_ne!(
        rows[" outlook.exe"].routine_id,
        owners.records["outlook"].routine_id
    );
    assert_eq!(
        rows["outlook.exe"].routine_id,
        owners.records["outlook.exe"].routine_id
    );
    for original in owners.records.values() {
        assert!(
            db.get_cf_exact(cf::CF_KV, &ledger_key(&original.routine_id))
                .expect("ledger read")
                .is_some(),
            "identity ownership must never be garbage collected"
        );
    }
}
