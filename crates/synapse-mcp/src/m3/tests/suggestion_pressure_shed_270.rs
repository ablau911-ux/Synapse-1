//! #270: the suggestion write gates (suggestion_accept for execution and
//! suggestion_tick) refuse a pressure-shed write with `STORAGE_WRITE_SHED` like
//! `routine_mine` (#165), same data shape, nothing written; dry runs write
//! nothing and are not refused by the gates.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

use crate::m3::tests::pressure_shed_support_270::{assert_shed, force_pressure, snapshot};

const SUGGESTION_ID: &str = "sg1-00000000000000000000000000000270";

fn live_suggestion() -> SuggestionRecord {
    SuggestionRecord {
        record_version: SUGGESTION_RECORD_VERSION,
        suggestion_id: SUGGESTION_ID.to_owned(),
        routine_id: "rt1-00000000000000cc".to_owned(),
        source: SuggestionSource::RoutineIntent,
        source_event_id: None,
        label: None,
        offer: None,
        mitigation: None,
        created_ts_ns: 1,
        expiry_ts_ns: u64::MAX / 2,
        status: SuggestionStatus::Live,
        confidence: 0.9,
        matched_prefix_len: 1,
        total_steps: 2,
        remaining_step_count: 1,
        proposed_plan_ref: None,
        resolved_ts_ns: None,
        resolution_note: None,
    }
}

fn db_with_live_suggestion() -> (tempfile::TempDir, Arc<Db>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), synapse_core::SCHEMA_VERSION).expect("db"));
    write_suggestion(&db, &live_suggestion()).expect("seed live suggestion");
    (dir, db)
}

#[test]
fn must_fail_suggestion_accept_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = db_with_live_suggestion();
    force_pressure(&db);
    let before = snapshot(&db);
    let error =
        accept_suggestion_for_execution(&db, SUGGESTION_ID, 5, "plan/270", "exec/270", false)
            .expect_err("pressure must refuse persisting the acceptance");
    assert_shed(&error, "suggestion_accept");
    assert_eq!(
        snapshot(&db),
        before,
        "the suggestion stays live and unchanged"
    );
}

/// Guard: a dry-run acceptance persists nothing, so the gate must admit it.
#[test]
fn must_pass_suggestion_accept_dry_run_is_admitted_under_pressure_270() {
    let (_dir, db) = db_with_live_suggestion();
    force_pressure(&db);
    let before = snapshot(&db);
    let record =
        accept_suggestion_for_execution(&db, SUGGESTION_ID, 5, "plan/270", "exec/270", true)
            .expect("a dry run is not a write");
    assert_eq!(record.status, SuggestionStatus::Accepted);
    assert_eq!(snapshot(&db), before);
}

#[test]
fn must_fail_suggestion_tick_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = db_with_live_suggestion();
    force_pressure(&db);
    let before = snapshot(&db);
    let params = SuggestionTickParams {
        dry_run: false,
        ..SuggestionTickParams::default()
    };
    let error = suggestion_tick(&db, &params).expect_err("pressure must refuse the tick write");
    assert_shed(&error, "suggestion_tick");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
}

/// Guard: the gate only guards persisting ticks; a dry run is not refused by it.
#[test]
fn must_pass_suggestion_tick_dry_run_is_not_refused_by_the_pressure_gate_270() {
    let (_dir, db) = db_with_live_suggestion();
    force_pressure(&db);
    let params = SuggestionTickParams {
        dry_run: true,
        ..SuggestionTickParams::default()
    };
    if let Err(error) = suggestion_tick(&db, &params) {
        assert!(
            !error.message.contains("disk pressure"),
            "a dry run must not be shed: {error:?}"
        );
    }
}
