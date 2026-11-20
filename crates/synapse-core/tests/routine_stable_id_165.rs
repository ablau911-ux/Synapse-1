//! Regression tests for #165: routine ids must be content-derived and stable
//! across mining runs, so operator state and armed automations never attach
//! to a different routine after re-mining.

use std::collections::BTreeMap;

use synapse_core::episodes::episode_id;
use synapse_core::routine_identity::match_routines_by_signature;
use synapse_core::routines::{MiningDay, RoutineMiningConfig, mine_routines};
use synapse_core::types::{
    EPISODE_RECORD_VERSION, EpisodeBoundary, EpisodeRecord, RoutineDowClass, RoutineRecord,
    TimelineActor,
};

const BASE: u64 = 1_700_000_000_000_000_000;
const DAY_NS: u64 = 86_400_000_000_000;
const MIN_NS: u64 = 60_000_000_000;

fn episode(day_start: u64, minute: u32, duration_min: u32, app: &str, doc: &str) -> EpisodeRecord {
    let start = day_start + u64::from(minute) * MIN_NS;
    EpisodeRecord {
        record_version: EPISODE_RECORD_VERSION,
        ts_ns: start,
        episode_id: episode_id(start, &TimelineActor::Human, Some(app), Some(doc)),
        start_ts_ns: start,
        end_ts_ns: start + u64::from(duration_min) * MIN_NS,
        actor: TimelineActor::Human,
        app: Some(app.to_owned()),
        document: Some(doc.to_owned()),
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

/// One planted routine (three steps near 08:45) on each requested
/// `(day_index, weekday)`. `apps`/`docs` let a test vary the spelling or the
/// content of the steps.
fn days_for(plan: &[(u64, u8)], apps: [&str; 3], docs: [&str; 3]) -> Vec<MiningDay> {
    plan.iter()
        .map(|&(index, weekday)| {
            let day_start = BASE + index * DAY_NS;
            MiningDay {
                day_start_ns: day_start,
                day_end_ns: day_start + DAY_NS,
                weekday,
                episodes: vec![
                    episode(day_start, 525, 10, apps[0], docs[0]),
                    episode(day_start, 535, 20, apps[1], docs[1]),
                    episode(day_start, 555, 5, apps[2], docs[2]),
                ],
            }
        })
        .collect()
}

fn weekday_plan() -> Vec<(u64, u8)> {
    (0..10_u64)
        .map(|k| (k, u8::try_from(k % 5).unwrap_or(0)))
        .collect()
}

/// Weekday plan plus four weekend days: same routine, but its day-of-week
/// class drifts from Weekdays to Daily.
fn grown_plan() -> Vec<(u64, u8)> {
    let mut plan = weekday_plan();
    plan.extend([(10, 5), (11, 6), (12, 5), (13, 6)]);
    plan
}

const APPS: [&str; 3] = ["outlook.exe", "excel.exe", "teams.exe"];
const DOCS: [&str; 3] = ["inbox", "report.xlsx", "chat"];

fn mine(days: &[MiningDay], at: u64) -> Vec<RoutineRecord> {
    mine_routines(days, at, &RoutineMiningConfig::default())
        .unwrap_or_else(|error| panic!("mining failed: {error}"))
        .routines
}

/// `step identity list -> routine` for the maximal three-step routine(s).
fn by_steps(routines: &[RoutineRecord]) -> BTreeMap<String, &RoutineRecord> {
    routines
        .iter()
        .filter(|routine| routine.steps.len() == 3)
        .map(|routine| {
            let key = routine
                .steps
                .iter()
                .map(|step| format!("{}|{}", step.app, step.document.clone().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(">");
            (key, routine)
        })
        .collect()
}

#[test]
fn must_pass_same_routine_mined_twice_keeps_its_id_even_when_dow_class_drifts() {
    let first = mine(&days_for(&weekday_plan(), APPS, DOCS), BASE);
    let second = mine(&days_for(&grown_plan(), APPS, DOCS), BASE + 20 * DAY_NS);
    let first_map = by_steps(&first);
    let second_map = by_steps(&second);
    assert!(!first_map.is_empty(), "first run must mine the routine");
    assert_eq!(
        first_map.keys().collect::<Vec<_>>(),
        second_map.keys().collect::<Vec<_>>(),
        "both runs must mine the same routine templates"
    );
    for (key, a) in &first_map {
        let b = second_map[key];
        assert_eq!(a.dow_class, RoutineDowClass::Weekdays, "precondition");
        assert_eq!(b.dow_class, RoutineDowClass::Daily, "precondition: drift");
        assert_eq!(a.routine_id, b.routine_id, "id drifted for {key}");
    }
}

#[test]
fn must_pass_cosmetic_spelling_differences_do_not_change_the_id() {
    let plain = mine(&days_for(&weekday_plan(), APPS, DOCS), BASE);
    let cosmetic = mine(
        &days_for(
            &weekday_plan(),
            [" Outlook.EXE ", "EXCEL.exe", "teams  "],
            ["Inbox ", " Report.XLSX", "chat"],
        ),
        BASE,
    );
    let plain_ids: Vec<&str> = {
        let mut ids: Vec<&str> = by_steps(&plain)
            .values()
            .map(|routine| routine.routine_id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    };
    let cosmetic_ids: Vec<&str> = {
        let mut ids: Vec<&str> = by_steps(&cosmetic)
            .values()
            .map(|routine| routine.routine_id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    };
    assert!(!plain_ids.is_empty());
    assert_eq!(plain_ids, cosmetic_ids);
}

#[test]
fn must_fail_a_different_routine_gets_a_different_id() {
    let original = mine(&days_for(&weekday_plan(), APPS, DOCS), BASE);
    let other = mine(
        &days_for(
            &weekday_plan(),
            ["outlook.exe", "word.exe", "teams.exe"],
            ["inbox", "letter.docx", "chat"],
        ),
        BASE,
    );
    let original_ids: Vec<&str> = original.iter().map(|r| r.routine_id.as_str()).collect();
    assert!(!original_ids.is_empty() && !other.is_empty());
    assert!(
        other.iter().any(|routine| routine.steps.len() == 3),
        "the different template must mine a 3-step routine, else the id check is vacuous"
    );
    for routine in &other {
        if routine.steps.len() == 3 {
            assert!(
                !original_ids.contains(&routine.routine_id.as_str()),
                "different template reused id {}",
                routine.routine_id
            );
        }
    }
}

#[test]
fn must_pass_legacy_ids_are_paired_to_the_remined_routine_by_signature() {
    let fresh = mine(&days_for(&grown_plan(), APPS, DOCS), BASE);
    let mut legacy = mine(&days_for(&weekday_plan(), APPS, DOCS), BASE);
    for (ordinal, routine) in legacy.iter_mut().enumerate() {
        routine.routine_id = format!("rt1-{ordinal:016x}");
    }
    let pairs = match_routines_by_signature(&legacy, &fresh);
    assert!(!pairs.is_empty(), "legacy routines must be matched");
    for (old_id, new_id) in &pairs {
        let old = legacy
            .iter()
            .find(|r| &r.routine_id == old_id)
            .expect("old routine");
        let new = fresh
            .iter()
            .find(|r| &r.routine_id == new_id)
            .expect("new routine");
        assert_eq!(
            old.steps, new.steps,
            "{old_id} bound to a different routine"
        );
    }
    assert_eq!(pairs.len(), legacy.len(), "every legacy routine re-mined");
}
