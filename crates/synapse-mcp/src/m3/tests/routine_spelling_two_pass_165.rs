//! #165: persisted identities belong to raw spellings when anchors cross or ties vanish.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "routine_spelling_three_pass_165.rs"]
mod routine_spelling_three_pass_165;

use super::*;
use crate::m3::routines::{RoutineMineParams, mine_and_store_routines};
use chrono::{Datelike, Duration, TimeZone};
use std::collections::{BTreeMap, BTreeSet};
use synapse_core::routine_identity::{match_routines_by_signature, routine_signature};

const MINUTE_NS: u64 = 60_000_000_000;

// Ten weekdays per spelling, with disjoint days so neither pattern suppresses
// the other as a subpattern. Replace only source episodes between mining passes.
fn replace_episodes(
    db: &Db,
    exe_minute: Option<u32>,
    bare_minute: Option<u32>,
) -> RoutineMineParams {
    // Pressure compaction applies the real 90-day episode TTL. Keep all source
    // days recent while retaining the original 44-day mining interval.
    let end = chrono::Local::now().date_naive();
    let first = end - Duration::days(44);
    let rows = spelling_episode_rows(first, exe_minute, bare_minute);
    let old_keys = db
        .scan_cf_prefix(cf::CF_EPISODES, b"")
        .expect("read source keys")
        .into_iter()
        .map(|(key, _)| key);
    db.mutate_batch_pressure_bypass(cf::CF_EPISODES, old_keys, rows)
        .expect("replace source episodes");
    let expected = usize::from(exe_minute.is_some()) * 30 + usize::from(bare_minute.is_some()) * 30;
    assert_eq!(
        db.scan_cf_prefix(cf::CF_EPISODES, b"")
            .expect("read source episodes")
            .len(),
        expected
    );
    let end_ts_ns = chrono::Local
        .with_ymd_and_hms(end.year(), end.month(), end.day(), 0, 0, 0)
        .single()
        .expect("fixture end date")
        .timestamp_nanos_opt()
        .expect("fixture end timestamp")
        .try_into()
        .expect("positive end timestamp");
    RoutineMineParams {
        end_ts_ns: Some(end_ts_ns),
        ..Default::default()
    }
}

fn spelling_episode_rows(
    first: chrono::NaiveDate,
    exe_minute: Option<u32>,
    bare_minute: Option<u32>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let weekdays: Vec<_> = (0..40)
        .map(|offset| first + Duration::days(offset))
        .filter(|day| day.weekday().num_days_from_monday() < 5)
        .take(20)
        .collect();
    let mut rows = Vec::new();
    for (index, day) in weekdays.iter().enumerate() {
        let (app, minute) = if index < 10 {
            ("outlook.exe", exe_minute)
        } else {
            ("outlook", bare_minute)
        };
        let Some(minute) = minute else { continue };
        let midnight: u64 = chrono::Local
            .with_ymd_and_hms(day.year(), day.month(), day.day(), 0, 0, 0)
            .single()
            .expect("unambiguous local fixture midnight")
            .timestamp_nanos_opt()
            .expect("fixture timestamp")
            .try_into()
            .expect("positive timestamp");
        for (offset, app) in [(0, app), (10, "excel.exe"), (20, "teams.exe")] {
            let start = midnight + u64::from(minute + offset) * MINUTE_NS;
            let ep = episode(start, app);
            rows.push((
                synapse_storage::episodes::episode_key(start, 0),
                encode_json(&ep).expect("encode source episode"),
            ));
        }
    }
    rows
}

fn stored_routines(db: &Db) -> Vec<RoutineRecord> {
    db.scan_cf_prefix(cf::CF_ROUTINES, b"")
        .expect("read persisted routines")
        .into_iter()
        .map(|(_, bytes)| synapse_storage::decode_json(&bytes).expect("decode persisted routine"))
        .collect()
}

fn by_spelling(rows: &[RoutineRecord]) -> BTreeMap<String, RoutineRecord> {
    let targets: Vec<_> = rows.iter().filter(|row| row.steps.len() == 3).collect();
    let result: BTreeMap<_, _> = targets
        .iter()
        .map(|row| {
            assert_eq!(row.support_days, 10);
            (row.steps[0].app.clone(), (**row).clone())
        })
        .collect();
    assert_eq!(
        result.len(),
        targets.len(),
        "one maximal routine per raw spelling"
    );
    let ids: BTreeSet<_> = targets.iter().map(|row| &row.routine_id).collect();
    assert_eq!(
        ids.len(),
        targets.len(),
        "persisted identities must remain unique"
    );
    result
}

fn first_pass(db: &Arc<Db>, exe_minute: u32, bare_minute: u32) -> BTreeMap<String, RoutineRecord> {
    let params = replace_episodes(db, Some(exe_minute), Some(bare_minute));
    mine_and_store_routines(db, &params).expect("first real mine and store");
    let before = by_spelling(&stored_routines(db));
    assert_eq!(before.len(), 2);
    assert_eq!(before["outlook.exe"].mean_minute_of_day, exe_minute);
    assert_eq!(before["outlook"].mean_minute_of_day, bare_minute);
    assert_eq!(
        routine_signature(
            before["outlook.exe"].granularity,
            &before["outlook.exe"].steps
        ),
        routine_signature(before["outlook"].granularity, &before["outlook"].steps)
    );
    for (app, row) in &before {
        let lifecycle = if app == "outlook.exe" {
            RoutineLifecycle::Disabled
        } else {
            RoutineLifecycle::Confirmed
        };
        let mut saved = state(&row.routine_id, lifecycle, app);
        saved.accept_count = if app == "outlook.exe" { 7 } else { 3 };
        put(
            db,
            cf::CF_ROUTINE_STATE,
            routine_codec::routine_state_key(&row.routine_id).expect("state key"),
            &saved,
        );
        let read = load_state_row(db, &row.routine_id)
            .expect("read seeded state")
            .expect("state exists");
        assert_eq!(read, saved);
    }
    assert!(
        db.get_cf_exact(cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes())
            .expect("read migration marker")
            .is_some(),
        "second pass must exercise recurrent reconciliation"
    );
    before
}

fn assert_retained(db: &Db, old: &RoutineRecord, fresh: &RoutineRecord) {
    assert_eq!(fresh.steps, old.steps);
    assert_eq!(
        fresh.routine_id, old.routine_id,
        "raw spelling inherited sibling identity"
    );
    let saved = load_state_row(db, &fresh.routine_id)
        .expect("read final state")
        .expect("final state exists");
    let exe = old.steps[0].app == "outlook.exe";
    assert_eq!(saved.routine_id, old.routine_id);
    assert_eq!(
        saved.label.as_deref(),
        Some(old.steps[0].app.as_str()),
        "operator label transferred to sibling"
    );
    assert_eq!(
        saved.lifecycle,
        if exe {
            RoutineLifecycle::Disabled
        } else {
            RoutineLifecycle::Confirmed
        }
    );
    assert_eq!(saved.accept_count, if exe { 7 } else { 3 });
    assert!(saved.present_in_last_mine);
}

#[test]
fn must_fail_crossed_minutes_cannot_swap_persisted_ids_and_state_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
    let before = first_pass(&db, 525, 526);
    let params = replace_episodes(&db, Some(526), Some(525));

    // Mine the same source in an empty identity DB to expose unreconciled
    // candidates for the real matcher, without fabricating ids or records.
    let raw_dir = tempfile::tempdir().expect("raw tempdir");
    let raw_db = Arc::new(Db::open(raw_dir.path(), SCHEMA_VERSION).expect("open raw db"));
    let raw_params = replace_episodes(&raw_db, Some(526), Some(525));
    let raw = mine_and_store_routines(&raw_db, &raw_params).expect("real second-pass candidates");
    let raw_by_app = by_spelling(&raw.routines);
    assert_eq!(raw_by_app.len(), 2);
    let old: Vec<_> = before.values().cloned().collect();
    let pairs = match_routines_by_signature(&old, &raw.routines);
    mine_and_store_routines(&db, &params).expect("second real mine and store");
    let after = by_spelling(&stored_routines(&db));
    assert_eq!(after.len(), 2);
    assert_eq!(after["outlook.exe"].mean_minute_of_day, 526);
    assert_eq!(after["outlook"].mean_minute_of_day, 525);
    for (app, old) in &before {
        assert_retained(&db, old, &after[app]);
    }
    // Exact ids legitimately produce no pair; changed ids must map to the SAME
    // spelling, including a two-cycle when the anchor-derived ids cross.
    for (app, previous) in &before {
        let candidate = &raw_by_app[app];
        if previous.routine_id != candidate.routine_id {
            assert!(
                pairs.contains(&(previous.routine_id.clone(), candidate.routine_id.clone())),
                "matcher failed to retain raw spelling {app}: {pairs:?}"
            );
        }
    }
}

#[test]
fn must_pass_unchanged_minutes_preserve_each_persisted_id_and_state_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
    let before = first_pass(&db, 525, 526);
    let params = replace_episodes(&db, Some(525), Some(526));
    mine_and_store_routines(&db, &params).expect("second unchanged mine");
    let after = by_spelling(&stored_routines(&db));
    assert_eq!(after.len(), 2);
    for (app, old) in &before {
        assert_retained(&db, old, &after[app]);
    }
}

#[test]
fn must_fail_shared_minute_deletion_cannot_transfer_deleted_sibling_state_165() {
    // Exercise BOTH survivors: candidate ordering must not decide ownership.
    for survivor in ["outlook.exe", "outlook"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
        let before = first_pass(&db, 525, 525);
        let params = replace_episodes(
            &db,
            (survivor == "outlook.exe").then_some(525),
            (survivor == "outlook").then_some(525),
        );
        mine_and_store_routines(&db, &params).expect("second mine with one spelling absent");
        let after = by_spelling(&stored_routines(&db));
        assert_eq!(after.len(), 1);
        assert_retained(&db, &before[survivor], &after[survivor]);
        let removed = if survivor == "outlook.exe" {
            "outlook"
        } else {
            "outlook.exe"
        };
        let absent = load_state_row(&db, &before[removed].routine_id)
            .expect("read disappeared state")
            .expect("operator state retained");
        assert!(
            !absent.present_in_last_mine,
            "deleted spelling cannot claim survivor presence"
        );
        assert_eq!(absent.label.as_deref(), Some(removed));
    }
}
