//! #165: recurrent reconciliation after migration, plus exact tolerance edges.
#[path = "routine_spelling_two_pass_165.rs"]
mod routine_spelling_two_pass_165;

use super::*;
use crate::m3::routines::{RoutineMineParams, mine_and_store_routines};
use synapse_core::routine_identity::reconcile_routine_identities;
use synapse_core::types::{EPISODE_RECORD_VERSION, EpisodeBoundary, EpisodeRecord, TimelineActor};

#[test]
fn must_pass_reconciliation_includes_circular_tolerance_boundary_165() {
    let old = routine(LEGACY_EXCEL, &["excel"], 1435);
    let mut fresh = vec![routine(LEGACY_VANISHED, &["excel"], 5)];
    fresh[0].tolerance_minutes = 2;
    assert_ne!(old.routine_id, fresh[0].routine_id);
    assert_eq!(old.tolerance_minutes, 10);
    reconcile_routine_identities(&[old], &mut fresh);
    assert_eq!(fresh[0].routine_id, LEGACY_EXCEL);
    assert_eq!(fresh[0].mean_minute_of_day, 5);
}

#[test]
fn must_fail_distant_or_already_claimed_schedule_cannot_steal_identity_165() {
    let old = routine(LEGACY_EXCEL, &["excel"], 1435);
    let mut fresh = vec![routine(LEGACY_VANISHED, &["excel"], 6)];
    let before = fresh.clone();
    reconcile_routine_identities(std::slice::from_ref(&old), &mut fresh);
    assert_eq!(fresh, before, "eleven minutes exceeds ten-minute tolerance");
    let exact = routine(LEGACY_VANISHED, &["excel"], 1436);
    let mut fresh = vec![exact.clone()];
    reconcile_routine_identities(&[old, exact.clone()], &mut fresh);
    assert_eq!(
        fresh,
        vec![exact],
        "exact identity claims itself before near matching"
    );
}

fn episode(start: u64, app: &str) -> EpisodeRecord {
    EpisodeRecord {
        record_version: EPISODE_RECORD_VERSION,
        ts_ns: start,
        episode_id: synapse_core::episodes::episode_id(
            start,
            &TimelineActor::Human,
            Some(app),
            None,
        ),
        start_ts_ns: start,
        end_ts_ns: start + 600_000_000_000,
        actor: TimelineActor::Human,
        app: Some(app.into()),
        document: None,
        url: None,
        title_first: None,
        title_last: None,
        distinct_title_count: 1,
        row_count: 4,
        keystroke_count: 10,
        click_count: 2,
        interruption_count: 0,
        interrupted_ms: 0,
        started_because: EpisodeBoundary::AppSwitch,
        ended_because: EpisodeBoundary::AppSwitch,
    }
}

fn episodes(db: &Db) -> RoutineMineParams {
    use chrono::TimeZone;
    let mut rows = Vec::new();
    for day in 1..=14 {
        let start: u64 = chrono::Local
            .with_ymd_and_hms(2026, 4, day, 15, 0, 0)
            .single()
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap()
            .try_into()
            .unwrap();
        for (ordinal, app) in ["outlook.exe", "excel.exe"].iter().enumerate() {
            let ep = episode(start + ordinal as u64 * 600_000_000_000, app);
            rows.push((
                synapse_storage::episodes::episode_key(ep.start_ts_ns, 0),
                encode_json(&ep).unwrap(),
            ));
        }
    }
    db.put_batch_pressure_bypass(cf::CF_EPISODES, rows).unwrap();
    assert_eq!(db.scan_cf_prefix(cf::CF_EPISODES, b"").unwrap().len(), 28);
    RoutineMineParams {
        end_ts_ns: Some(1_778_803_200_000_000_000),
        ..Default::default()
    }
}

fn persisted_fixture() -> (tempfile::TempDir, Arc<Db>, RoutineMineParams) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
    let params = episodes(&db);
    let first = mine_and_store_routines(&db, &params).unwrap();
    let mut old = first
        .routines
        .iter()
        .find(|r| r.steps.len() == 2)
        .unwrap()
        .clone();
    assert!(
        db.get_cf_exact(cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes())
            .unwrap()
            .is_some()
    );
    clear_preledger_ownership_165(&db);
    old.routine_id = LEGACY_EXCEL.into();
    replace_legacy_record_165(&db, &old);
    seed_legacy_references_165(&db);
    (dir, db, params)
}

fn clear_preledger_ownership_165(db: &Db) {
    // Model a genuine pre-ledger snapshot. Keeping the freshly generated
    // ledger here would correctly give its recorded owner precedence over
    // this synthetic replacement id, contradicting the fixture's premise.
    let ledger_keys = db
        .scan_cf_prefix(
            cf::CF_KV,
            crate::m3::routine_identity_ledger::ROUTINE_IDENTITY_PREFIX.as_bytes(),
        )
        .unwrap()
        .into_iter()
        .map(|(key, _)| key);
    db.mutate_batch_pressure_bypass(cf::CF_KV, ledger_keys, Vec::<(Vec<u8>, Vec<u8>)>::new())
        .unwrap();
}

fn replace_legacy_record_165(db: &Db, old: &RoutineRecord) {
    let keys = db
        .scan_cf_prefix(cf::CF_ROUTINES, b"")
        .unwrap()
        .into_iter()
        .map(|r| r.0);
    db.mutate_batch_pressure_bypass(
        cf::CF_ROUTINES,
        keys,
        [(
            routine_codec::routine_key(LEGACY_EXCEL).unwrap(),
            encode_json(old).unwrap(),
        )],
    )
    .unwrap();
}

fn seed_legacy_references_165(db: &Db) {
    put(
        db,
        cf::CF_ROUTINE_STATE,
        routine_codec::routine_state_key(LEGACY_EXCEL).unwrap(),
        &state(
            LEGACY_EXCEL,
            RoutineLifecycle::Confirmed,
            "Keep operator label",
        ),
    );
    put(
        db,
        cf::CF_KV,
        armed_routine_key(LEGACY_EXCEL).into_bytes(),
        &armed(LEGACY_EXCEL),
    );
    put(
        db,
        cf::CF_KV,
        routine_automation_key(LEGACY_EXCEL).into_bytes(),
        &automation(LEGACY_EXCEL),
    );
    assert_eq!(
        load_state_row(db, LEGACY_EXCEL)
            .unwrap()
            .unwrap()
            .label
            .as_deref(),
        Some("Keep operator label")
    );
}

#[test]
fn must_pass_remining_after_marker_preserves_persisted_identity_and_operator_state_165() {
    let (_dir, db, params) = persisted_fixture();
    let armed_before = db
        .get_cf_exact(cf::CF_KV, armed_routine_key(LEGACY_EXCEL).as_bytes())
        .unwrap();
    let auto_before = db
        .get_cf_exact(cf::CF_KV, routine_automation_key(LEGACY_EXCEL).as_bytes())
        .unwrap();
    let result = mine_and_store_routines(&db, &params).unwrap();
    assert!(result.routines.iter().any(|r| r.routine_id == LEGACY_EXCEL));
    assert!(
        db.get_cf_exact(
            cf::CF_ROUTINES,
            &routine_codec::routine_key(LEGACY_EXCEL).unwrap()
        )
        .unwrap()
        .is_some()
    );
    let state = load_state_row(&db, LEGACY_EXCEL).unwrap().unwrap();
    assert_eq!(state.lifecycle, RoutineLifecycle::Confirmed);
    assert_eq!(state.label.as_deref(), Some("Keep operator label"));
    assert!(state.present_in_last_mine);
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, armed_routine_key(LEGACY_EXCEL).as_bytes())
            .unwrap(),
        armed_before
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, routine_automation_key(LEGACY_EXCEL).as_bytes())
            .unwrap(),
        auto_before
    );
}

#[test]
fn must_fail_invalid_remine_range_cannot_rebind_persisted_references_165() {
    let (_dir, db, mut params) = persisted_fixture();
    let before = db.scan_cf_prefix(cf::CF_ROUTINES, b"").unwrap();
    let references = db.scan_cf_prefix(cf::CF_KV, b"").unwrap();
    params.start_ts_ns = params.end_ts_ns;
    let error = mine_and_store_routines(&db, &params).unwrap_err();
    assert_eq!(
        error.data.as_ref().unwrap()["code"],
        synapse_core::error_codes::TOOL_PARAMS_INVALID
    );
    assert_eq!(db.scan_cf_prefix(cf::CF_ROUTINES, b"").unwrap(), before);
    assert_eq!(db.scan_cf_prefix(cf::CF_KV, b"").unwrap(), references);
}
