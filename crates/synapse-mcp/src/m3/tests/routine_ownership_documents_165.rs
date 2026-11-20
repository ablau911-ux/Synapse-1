//! Document spellings obey the same ownership rules as app spellings.
use super::*;

fn document_source(db: &Db, left: Option<&str>, right: Option<&str>) -> RoutineMineParams {
    let params = replace_episodes(db, left.map(|_| 525), right.map(|_| 525));
    let rows = db
        .scan_cf_prefix(cf::CF_EPISODES, b"")
        .unwrap()
        .into_iter()
        .map(|(key, bytes)| {
            let mut row: synapse_core::types::EpisodeRecord =
                synapse_storage::decode_json(&bytes).unwrap();
            let document = match row.app.as_deref() {
                Some("outlook.exe") => left,
                Some("outlook") => right,
                _ => None,
            };
            if let Some(document) = document {
                row.app = Some("outlook.exe".into());
                row.document = Some(document.into());
                row.episode_id = synapse_core::episodes::episode_id(
                    row.start_ts_ns,
                    &row.actor,
                    row.app.as_deref(),
                    row.document.as_deref(),
                );
            }
            (key, encode_json(&row).unwrap())
        });
    db.put_batch_pressure_bypass(cf::CF_EPISODES, rows).unwrap();
    params
}

fn documents(db: &Db) -> BTreeMap<String, RoutineRecord> {
    let rows: BTreeMap<_, _> = stored_routines(db)
        .into_iter()
        .filter(|row| row.granularity == RoutineGranularity::AppDocument && row.steps.len() == 3)
        .map(|row| {
            assert_eq!(row.support_days, 10);
            (
                row.steps[0].document.clone().expect("document fixture"),
                row,
            )
        })
        .collect();
    let ids: BTreeSet<_> = rows.values().map(|row| &row.routine_id).collect();
    assert_eq!(ids.len(), rows.len(), "document identities collided");
    rows
}

#[test]
fn must_fail_returning_document_spelling_cannot_inherit_current_sibling_state_165() {
    for (left, right) in [(r"c:\inbox", "c:/inbox"), ("inbox ", "inbox")] {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
        let params = document_source(&db, Some(left), Some(right));
        mine_and_store_routines(&db, &params).unwrap();
        let original = documents(&db);
        assert_eq!(original.len(), 2);
        let (saved, refs) = seed_owner_rows(&db, &original[right].routine_id, true);
        let params = document_source(&db, Some(left), None);
        mine_and_store_routines(&db, &params).unwrap();
        let params = document_source(&db, None, Some(right));
        mine_and_store_routines(&db, &params).unwrap();
        let returned = documents(&db);
        assert_eq!(returned.len(), 1);
        assert_eq!(returned[right].routine_id, original[right].routine_id);
        assert_operator_payload(
            &load_state_row(&db, &saved.routine_id).unwrap().unwrap(),
            &saved,
        );
        assert_eq!(references_with_taint(&db, &saved.routine_id), refs);
    }
}

#[test]
fn must_pass_unambiguous_legacy_document_cosmetics_retain_identity_165() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
    let params = document_source(&db, Some(r"c:\inbox"), None);
    mine_and_store_routines(&db, &params).unwrap();
    let mut old = documents(&db)[r"c:\inbox"].clone();
    old.routine_id = LEGACY_EXCEL.into();
    old.steps[0].document = Some(" C:/INBOX ".into());
    rewrite_current(&db, &old);
    forget_ledger(&db);
    let (saved, refs) = seed_owner_rows(&db, LEGACY_EXCEL, true);
    mine_and_store_routines(&db, &params).unwrap();
    assert_eq!(documents(&db)[r"c:\inbox"].routine_id, LEGACY_EXCEL);
    assert_operator_payload(&load_state_row(&db, LEGACY_EXCEL).unwrap().unwrap(), &saved);
    assert_eq!(references_with_taint(&db, LEGACY_EXCEL), refs);
}

#[test]
fn must_fail_signature_delimiters_cannot_change_legacy_step_structure_165() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).unwrap());
    let params = document_source(&db, Some("b\u{1e}c"), None);
    mine_and_store_routines(&db, &params).unwrap();
    let candidate = documents(&db)["b\u{1e}c"].clone();
    let mut old = candidate.clone();
    old.routine_id = LEGACY_EXCEL.into();
    old.steps[0].app = "outlook\u{1e}b".into();
    old.steps[0].document = Some("c".into());
    assert_eq!(
        routine_signature(old.granularity, &old.steps),
        routine_signature(candidate.granularity, &candidate.steps),
        "collision precondition"
    );
    rewrite_current(&db, &old);
    forget_ledger(&db);
    let (saved, refs) = seed_owner_rows(&db, LEGACY_EXCEL, true);
    mine_and_store_routines(&db, &params).unwrap();
    let fresh = documents(&db)["b\u{1e}c"].clone();
    assert_ne!(
        fresh.routine_id, LEGACY_EXCEL,
        "signature text is not structural equivalence"
    );
    assert_candidate(&db, &fresh.routine_id);
    assert_operator_payload(&load_state_row(&db, LEGACY_EXCEL).unwrap().unwrap(), &saved);
    assert_eq!(references_with_taint(&db, LEGACY_EXCEL), refs);
}
