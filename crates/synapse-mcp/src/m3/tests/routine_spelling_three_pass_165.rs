//! #165: vanished ownership and shared-minute insertion, through the real miner.
#[path = "routine_ownership_return_165.rs"]
mod routine_ownership_return_165;

use super::*;
use synapse_core::routine_identity::routine_id;
use synapse_core::types::{RoutineFeedbackEvent, RoutineFeedbackOutcome, RoutineStateRecord};

fn replace_spellings(db: &Db, sibling: Option<&str>) -> RoutineMineParams {
    let params = replace_episodes(db, Some(525), sibling.map(|_| 525));
    if let Some(spelling) = sibling.filter(|spelling| *spelling != "outlook") {
        let rows = db
            .scan_cf_prefix(cf::CF_EPISODES, b"")
            .expect("read source episodes")
            .into_iter()
            .filter_map(|(key, bytes)| {
                let old: synapse_core::types::EpisodeRecord =
                    synapse_storage::decode_json(&bytes).expect("decode source episode");
                (old.app.as_deref() == Some("outlook")).then(|| {
                    let replacement = episode(old.start_ts_ns, spelling);
                    (key, encode_json(&replacement).expect("encode new spelling"))
                })
            });
        db.put_batch_pressure_bypass(cf::CF_EPISODES, rows)
            .expect("replace sibling source spelling");
    }
    params
}

fn operator_state(db: &Db, id: &str, exe: bool) -> RoutineStateRecord {
    let mut saved = state(
        id,
        if exe {
            RoutineLifecycle::Disabled
        } else {
            RoutineLifecycle::Confirmed
        },
        if exe {
            "Keep exe owner"
        } else {
            "Deleted bare owner"
        },
    );
    saved.accept_count = if exe { 7 } else { 3 };
    saved.decline_count = if exe { 2 } else { 5 };
    saved.ignore_count = if exe { 1 } else { 4 };
    saved.abandon_count = if exe { 6 } else { 8 };
    saved.consecutive_declines = if exe { 2 } else { 5 };
    saved.cooldown_level = if exe { 1 } else { 2 };
    saved.cooldown_until_ts_ns = Some(if exe { 100 } else { 200 });
    saved.feedback_events.push(RoutineFeedbackEvent {
        ts_ns: 1,
        outcome: if exe {
            RoutineFeedbackOutcome::Accepted
        } else {
            RoutineFeedbackOutcome::Declined
        },
        by: "operator".into(),
        note: saved.label.clone(),
    });
    put(
        db,
        cf::CF_ROUTINE_STATE,
        routine_codec::routine_state_key(id).expect("state key"),
        &saved,
    );
    assert_eq!(
        load_state_row(db, id).expect("read seeded state"),
        Some(saved.clone())
    );
    saved
}

fn assert_operator_payload(actual: &RoutineStateRecord, expected: &RoutineStateRecord) {
    assert_eq!(actual.routine_id, expected.routine_id);
    assert_eq!(
        actual.label, expected.label,
        "operator label changed owners"
    );
    assert_eq!(
        actual.lifecycle, expected.lifecycle,
        "operator lifecycle changed owners"
    );
    assert_eq!(
        actual.feedback_events, expected.feedback_events,
        "feedback changed owners"
    );
    assert_eq!(
        actual.feedback_events_truncated,
        expected.feedback_events_truncated
    );
    assert_eq!(actual.accept_count, expected.accept_count);
    assert_eq!(actual.decline_count, expected.decline_count);
    assert_eq!(actual.ignore_count, expected.ignore_count);
    assert_eq!(actual.abandon_count, expected.abandon_count);
    assert_eq!(actual.consecutive_declines, expected.consecutive_declines);
    assert_eq!(actual.cooldown_level, expected.cooldown_level);
    assert_eq!(actual.cooldown_until_ts_ns, expected.cooldown_until_ts_ns);
}

fn assert_candidate(db: &Db, id: &str) {
    let fresh = load_state_row(db, id)
        .expect("read newcomer state")
        .expect("newcomer state exists");
    assert_eq!(
        fresh.label, None,
        "newcomer inherited vanished/sibling operator label (#165)"
    );
    assert_eq!(
        fresh.lifecycle,
        RoutineLifecycle::Candidate,
        "newcomer inherited operator lifecycle (#165)"
    );
    assert!(fresh.present_in_last_mine);
    assert!(
        fresh.feedback_events.is_empty(),
        "newcomer inherited operator feedback (#165)"
    );
    assert_eq!(fresh.feedback_events_truncated, 0);
    assert_eq!(
        (
            fresh.accept_count,
            fresh.decline_count,
            fresh.ignore_count,
            fresh.abandon_count
        ),
        (0, 0, 0, 0)
    );
    assert_eq!((fresh.consecutive_declines, fresh.cooldown_level), (0, 0));
    assert_eq!(fresh.cooldown_until_ts_ns, None);
}

fn reference_bytes(db: &Db, id: &str) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        db.get_cf_exact(cf::CF_KV, armed_routine_key(id).as_bytes())
            .expect("read armed owner"),
        db.get_cf_exact(cf::CF_KV, routine_automation_key(id).as_bytes())
            .expect("read automation owner"),
    )
}

fn seed_references(db: &Db, id: &str) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    put(
        db,
        cf::CF_KV,
        armed_routine_key(id).into_bytes(),
        &armed(id),
    );
    put(
        db,
        cf::CF_KV,
        routine_automation_key(id).into_bytes(),
        &automation(id),
    );
    let bytes = reference_bytes(db, id);
    assert!(bytes.0.is_some() && bytes.1.is_some());
    bytes
}

fn assert_rt1(id: &str) {
    let hex = id
        .strip_prefix("rt1-")
        .expect("displaced id retains rt1 prefix");
    assert_eq!(hex.len(), 16);
    assert!(
        hex.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "displaced id must match rt1-[0-9a-f]{{16}}"
    );
}

fn ids(rows: &[RoutineRecord]) -> BTreeMap<String, String> {
    by_spelling(rows)
        .into_iter()
        .map(|(app, row)| (app, row.routine_id))
        .collect()
}

#[test]
fn must_fail_third_pass_newcomer_cannot_inherit_vanished_owner_state_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));

    // Review's exact table: ten disjoint days per spelling, same minute 525.
    let before = first_pass(&db, 525, 525);
    let bare = &before["outlook"];
    let exe = &before["outlook.exe"];
    let h0 = routine_id(bare.granularity, &bare.steps, 1965);
    let h1 = routine_id(exe.granularity, &exe.steps, 3405);
    assert_eq!(bare.routine_id, h0, "pass 1 bare spelling owns H0");
    assert_eq!(exe.routine_id, h1, "pass 1 exe spelling owns H1");
    let bare_state = operator_state(&db, &h0, false);
    let exe_state = operator_state(&db, &h1, true);
    let bare_refs = seed_references(&db, &h0);
    let exe_refs = seed_references(&db, &h1);

    let params = replace_spellings(&db, None);
    mine_and_store_routines(&db, &params).expect("pass 2 exe survivor only");
    let second = by_spelling(&stored_routines(&db));
    assert_eq!(second.len(), 1);
    assert_eq!(second["outlook.exe"].routine_id, h1);
    assert!(
        db.get_cf_exact(
            cf::CF_ROUTINES,
            &routine_codec::routine_key(&h0).expect("H0 key")
        )
        .expect("read vanished H0 routine")
        .is_none()
    );
    let vanished = load_state_row(&db, &h0)
        .expect("read H0 state")
        .expect("H0 state retained");
    assert!(
        !vanished.present_in_last_mine,
        "pass 2 must retain H0 state as unmined"
    );
    assert_operator_payload(&vanished, &bare_state);
    let survivor = load_state_row(&db, &h1)
        .expect("read H1 state")
        .expect("H1 state retained");
    assert!(survivor.present_in_last_mine);
    assert_operator_payload(&survivor, &exe_state);
    assert_eq!(reference_bytes(&db, &h0), bare_refs);
    assert_eq!(reference_bytes(&db, &h1), exe_refs);

    let params = replace_spellings(&db, Some(" outlook.exe"));
    mine_and_store_routines(&db, &params).expect("pass 3 leading-space newcomer plus exe");
    let third = by_spelling(&stored_routines(&db));
    assert_eq!(third.len(), 2);
    assert_eq!(third["outlook.exe"].routine_id, h1);
    let newcomer = &third[" outlook.exe"];
    assert_eq!(newcomer.mean_minute_of_day, 525);
    assert_eq!(
        routine_signature(newcomer.granularity, &newcomer.steps),
        routine_signature(bare.granularity, &bare.steps)
    );
    // Assert state before id so the existing defect fails on actual theft.
    assert_candidate(&db, &newcomer.routine_id);
    assert_ne!(
        newcomer.routine_id, h0,
        "vanished ownership must remain reserved"
    );
    assert_ne!(newcomer.routine_id, h1);
    assert_rt1(&newcomer.routine_id);
    assert_eq!(
        load_state_row(&db, &h0).expect("read retained H0 after pass 3"),
        Some(vanished)
    );
    let survivor = load_state_row(&db, &h1)
        .expect("read survivor after pass 3")
        .expect("survivor state");
    assert_operator_payload(&survivor, &exe_state);
    assert!(survivor.present_in_last_mine);
    assert_eq!(reference_bytes(&db, &h0), bare_refs);
    assert_eq!(reference_bytes(&db, &h1), exe_refs);
    assert_eq!(reference_bytes(&db, &newcomer.routine_id), (None, None));
}

#[test]
fn must_pass_shared_minute_insertion_displaces_newcomer_and_preserves_owner_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
    let params = replace_spellings(&db, None);
    mine_and_store_routines(&db, &params).expect("first mine exe only");
    let first = by_spelling(&stored_routines(&db));
    assert_eq!(first.len(), 1);
    let owner = &first["outlook.exe"];
    assert_eq!(
        owner.routine_id,
        routine_id(owner.granularity, &owner.steps, 1965)
    );
    let saved = operator_state(&db, &owner.routine_id, true);
    let refs = seed_references(&db, &owner.routine_id);
    assert!(
        db.get_cf_exact(cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes())
            .expect("read marker")
            .is_some()
    );

    let mut params = replace_spellings(&db, Some("outlook"));
    // Earlier-sorting bare spelling takes the owner's raw H0 tie slot.
    // Dry-run must compute the displaced id while leaving physical rows alone.
    let routines_before = db
        .scan_cf_prefix(cf::CF_ROUTINES, b"")
        .expect("snapshot routines");
    let state_before = db
        .scan_cf_prefix(cf::CF_ROUTINE_STATE, b"")
        .expect("snapshot state");
    let kv_before = db
        .scan_cf_prefix(cf::CF_KV, b"")
        .expect("snapshot references");
    params.dry_run = true;
    let preview = mine_and_store_routines(&db, &params).expect("dry-run insertion");
    let preview_ids = ids(&preview.routines);
    assert_eq!(preview_ids.len(), 2);
    assert_eq!(preview_ids["outlook.exe"], owner.routine_id);
    assert_ne!(
        preview_ids["outlook"], owner.routine_id,
        "unmatched newcomer must be displaced from owner's H0 (#165)"
    );
    assert_rt1(&preview_ids["outlook"]);
    assert_eq!(
        db.scan_cf_prefix(cf::CF_ROUTINES, b"")
            .expect("read routines after dry run"),
        routines_before
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_ROUTINE_STATE, b"")
            .expect("read state after dry run"),
        state_before
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, b"")
            .expect("read references after dry run"),
        kv_before
    );

    params.dry_run = false;
    mine_and_store_routines(&db, &params).expect("commit shared-minute insertion");
    let second_rows = stored_routines(&db);
    assert_eq!(
        ids(&second_rows),
        preview_ids,
        "dry-run and persisted identities must agree"
    );
    let second = by_spelling(&second_rows);
    assert_eq!(
        second.len(),
        2,
        "insertion must persist both records, never overwrite owner"
    );
    assert_eq!(second["outlook.exe"].steps, owner.steps);
    let newcomer = &second["outlook"];
    assert_candidate(&db, &newcomer.routine_id);
    let retained = load_state_row(&db, &owner.routine_id)
        .expect("read owner state")
        .expect("owner state");
    assert_operator_payload(&retained, &saved);
    assert!(retained.present_in_last_mine);
    assert_eq!(reference_bytes(&db, &owner.routine_id), refs);
    assert_eq!(reference_bytes(&db, &newcomer.routine_id), (None, None));

    mine_and_store_routines(&db, &params).expect("repeat same insertion source");
    assert_eq!(
        ids(&stored_routines(&db)),
        preview_ids,
        "displaced identity must survive repeated mining"
    );
    assert_candidate(&db, &newcomer.routine_id);
    let repeated = load_state_row(&db, &owner.routine_id)
        .expect("read repeated owner")
        .expect("owner state");
    assert_operator_payload(&repeated, &saved);
    assert!(repeated.present_in_last_mine);
    assert_eq!(reference_bytes(&db, &owner.routine_id), refs);
    assert_eq!(reference_bytes(&db, &newcomer.routine_id), (None, None));
}
