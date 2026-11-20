//! #165: raw candidates sharing a normalized signature must retain both identities.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn spellings(starts: &[u32]) -> Vec<RoutineRecord> {
    spelling_variants(starts, "outlook")
}

fn spelling_variants(starts: &[u32], alternate_app: &str) -> Vec<RoutineRecord> {
    let days: Vec<_> = (0..20)
        .map(|index| {
            let start = BASE + index * DAY;
            let app = if index < 10 {
                "outlook.exe"
            } else {
                alternate_app
            };
            let episodes = starts
                .iter()
                .flat_map(|minute| {
                    [
                        episode(start, *minute, app),
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
    mine_routines(&days, BASE + 21 * DAY, &RoutineMiningConfig::default())
        .expect("valid spelling fixtures")
        .routines
}

fn at_0845(rows: &[RoutineRecord]) -> Vec<&RoutineRecord> {
    rows.iter()
        .filter(|row| row.steps.len() == 3 && row.mean_minute_of_day == 525)
        .collect()
}

#[test]
fn must_fail_same_minute_spelling_candidates_cannot_silently_alias_ids_165() {
    let rows = spellings(&[525]);
    let targets = at_0845(&rows);
    assert_eq!(
        targets.len(),
        2,
        "criteria: different raw clusters remain distinct"
    );
    let apps: BTreeSet<_> = targets
        .iter()
        .map(|row| row.steps[0].app.as_str())
        .collect();
    // ASCII lowercasing preserves this suffix difference before grouping;
    // routine_signature removes the suffix, exercising the actual collision.
    assert_eq!(apps, BTreeSet::from(["outlook", "outlook.exe"]));
    let signatures: BTreeSet<_> = targets
        .iter()
        .map(|row| synapse_core::routine_identity::routine_signature(row.granularity, &row.steps))
        .collect();
    assert_eq!(
        signatures.len(),
        1,
        "fixture must hit normalized-signature collision"
    );
    for row in &targets {
        assert_eq!(row.support_days, 10);
        assert_eq!(row.occurrence_count, 10);
    }
    let ids: BTreeSet<_> = targets.iter().map(|row| &row.routine_id).collect();
    assert_eq!(
        ids.len(),
        2,
        "same-minute spellings must not overwrite one CF_ROUTINES key"
    );
}

#[test]
fn must_pass_case_only_spelling_variants_merge_before_grouping_165() {
    let rows = spelling_variants(&[525], "OUTLOOK.EXE");
    let targets = at_0845(&rows);
    assert_eq!(
        targets.len(),
        1,
        "case-only variants must form one preprocessed candidate"
    );
    let row = targets[0];
    assert_eq!(row.steps[0].app, "outlook.exe");
    assert_eq!(row.support_days, 20);
    assert_eq!(row.occurrence_count, 20);

    let lowercase_only = spelling_variants(&[525], "outlook.exe");
    let expected = at_0845(&lowercase_only);
    assert_eq!(expected.len(), 1);
    assert_eq!(
        row.routine_id, expected[0].routine_id,
        "case-only preprocessing must preserve the lowercase-only identity"
    );
}

#[test]
fn must_pass_spelling_ids_survive_other_cluster_insertion_and_deletion_165() {
    let identities = |rows: &[RoutineRecord]| -> BTreeMap<String, String> {
        let targets = at_0845(rows);
        assert_eq!(targets.len(), 2);
        targets
            .into_iter()
            .map(|row| (row.steps[0].app.clone(), row.routine_id.clone()))
            .collect()
    };
    let before = identities(&spellings(&[525]));
    assert_eq!(before, identities(&spellings(&[525])), "IDs must repeat");
    // Keep the inserted cluster beyond the default 120-minute split gap.
    let inserted = identities(&spellings(&[300, 525, 900]));
    let deleted = identities(&spellings(&[525, 900]));
    let restored = identities(&spellings(&[525]));
    assert_eq!(
        before, inserted,
        "inserting unrelated time clusters moved spelling identities"
    );
    assert_eq!(
        inserted, deleted,
        "deleting earlier cluster moved spelling identities"
    );
    assert_eq!(
        deleted, restored,
        "deleting later cluster moved spelling identities"
    );
}
