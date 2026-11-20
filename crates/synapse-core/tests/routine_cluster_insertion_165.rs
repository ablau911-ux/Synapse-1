//! Same signature at a new earlier time must not steal an existing identity.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use synapse_core::episodes::episode_id;
use synapse_core::routines::{MiningDay, RoutineMiningConfig, mine_routines};
use synapse_core::types::{
    EPISODE_RECORD_VERSION, EpisodeBoundary, EpisodeRecord, RoutineRecord, TimelineActor,
};

#[path = "cases/routine_spelling_collision_165.rs"]
mod routine_spelling_collision_165;

const BASE: u64 = 1_700_000_000_000_000_000;
const DAY: u64 = 86_400_000_000_000;
const MINUTE: u64 = 60_000_000_000;

fn episode(day: u64, minute: u32, app: &str) -> EpisodeRecord {
    let start = day + u64::from(minute) * MINUTE;
    EpisodeRecord {
        record_version: EPISODE_RECORD_VERSION,
        ts_ns: start,
        episode_id: episode_id(start, &TimelineActor::Human, Some(app), Some("fixture")),
        start_ts_ns: start,
        end_ts_ns: start + 10 * MINUTE,
        actor: TimelineActor::Human,
        app: Some(app.into()),
        document: Some("fixture".into()),
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

fn mine(starts: &[u32]) -> Vec<RoutineRecord> {
    let days: Vec<_> = (0..10)
        .map(|index| {
            let start = BASE + index * DAY;
            let episodes = starts
                .iter()
                .flat_map(|minute| {
                    [
                        episode(start, *minute, "outlook.exe"),
                        episode(start, minute + 10, "excel.exe"),
                        episode(start, minute + 20, "teams.exe"),
                    ]
                })
                .collect();
            MiningDay {
                day_start_ns: start,
                day_end_ns: start + DAY,
                weekday: (index % 5) as u8,
                episodes,
            }
        })
        .collect();
    mine_routines(&days, BASE + 11 * DAY, &RoutineMiningConfig::default())
        .expect("valid episodes")
        .routines
}

fn maximal_at(rows: &[RoutineRecord], minute: u32) -> &RoutineRecord {
    let matching: Vec<_> = rows
        .iter()
        .filter(|row| row.steps.len() == 3 && row.mean_minute_of_day == minute)
        .collect();
    assert_eq!(matching.len(), 1, "one closed maximal routine at {minute}");
    matching[0]
}

#[test]
fn must_pass_existing_cluster_keeps_id_when_earlier_cluster_appears_165() {
    let original = mine(&[900]);
    let expanded = mine(&[480, 900]);
    let old = maximal_at(&original, 900);
    let retained = maximal_at(&expanded, 900);
    let inserted = maximal_at(&expanded, 480);
    assert_eq!(old.steps, retained.steps);
    assert_eq!(old.steps, inserted.steps, "same-signature precondition");
    assert_eq!(old.support_days, 10);
    assert_eq!(retained.support_days, 10);
    assert_ne!(inserted.routine_id, retained.routine_id);
    assert_eq!(
        old.routine_id, retained.routine_id,
        "existing automation identity moved"
    );
}

#[test]
fn must_fail_removing_earlier_cluster_cannot_transfer_its_id_to_later_cluster_165() {
    let before = mine(&[480, 900]);
    let after = mine(&[900]);
    let earlier = maximal_at(&before, 480);
    let later = maximal_at(&before, 900);
    let survivor = maximal_at(&after, 900);
    assert_eq!(earlier.steps, later.steps);
    assert_ne!(earlier.routine_id, later.routine_id);
    assert_eq!(
        later.routine_id, survivor.routine_id,
        "deletion renumbered survivor"
    );
    assert_ne!(
        earlier.routine_id, survivor.routine_id,
        "removed routine state was rebound"
    );
}
