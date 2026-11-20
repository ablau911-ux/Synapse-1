//! Regression tests for #98 (re-review): `routine label`, `routine automate`
//! and the plan compiler must classify a routine id that is absent from both
//! `CF_ROUTINES` and `CF_ROUTINE_STATE` as `ROUTINE_NOT_FOUND`, and an id that
//! only has an operator state row as a `ROUTINE_NOT_MINED` precondition
//! failure — never `TOOL_PARAMS_INVALID` or `STORAGE_READ_FAILED`.

use super::*;

use synapse_core::SCHEMA_VERSION;
use synapse_core::types::{
    ROUTINE_RECORD_VERSION, RoutineDowClass, RoutineGranularity, RoutineRecord,
};
use synapse_core::types::{ROUTINE_STATE_RECORD_VERSION, RoutineLifecycle, RoutineStateRecord};

use crate::m3::plan::{RoutineCompilePlanParams, compile_routine_plan};

const ABSENT: &str = "rt1-00000000000000aa";
const STATE_ONLY: &str = "rt1-00000000000000bb";

fn state_row(id: &str) -> RoutineStateRecord {
    RoutineStateRecord {
        record_version: ROUTINE_STATE_RECORD_VERSION,
        routine_id: id.to_owned(),
        lifecycle: RoutineLifecycle::Confirmed,
        label: Some("operator label".to_owned()),
        created_ts_ns: 1,
        updated_ts_ns: 1,
        last_mined_ts_ns: None,
        present_in_last_mine: false,
        transitions: Vec::new(),
        transitions_truncated: 0,
        confidence_history: Vec::new(),
        confidence_history_truncated: 0,
        feedback_events: Vec::new(),
        feedback_events_truncated: 0,
        accept_count: 0,
        decline_count: 0,
        ignore_count: 0,
        abandon_count: 0,
        consecutive_declines: 0,
        cooldown_level: 0,
        cooldown_until_ts_ns: None,
    }
}

fn db_with_state_only_row() -> (tempfile::TempDir, Arc<Db>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
    let key = routine_codec::routine_state_key(STATE_ONLY).expect("state key");
    db.put_batch_pressure_bypass(
        cf::CF_ROUTINE_STATE,
        [(key, encode_json(&state_row(STATE_ONLY)).expect("encode"))],
    )
    .expect("put state row");
    (dir, db)
}

fn code_of(error: &ErrorData) -> String {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(|code| code.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn label_params(id: &str) -> RoutineLabelExportParams {
    RoutineLabelExportParams {
        routine_id: id.to_owned(),
        max_samples: None,
    }
}

fn plan_params(id: &str) -> RoutineCompilePlanParams {
    RoutineCompilePlanParams {
        routine_id: id.to_owned(),
        store: false,
    }
}

#[test]
fn must_fail_label_export_of_absent_routine_is_not_found() {
    let (_dir, db) = db_with_state_only_row();
    let error = export_routine_label(&db, &label_params(ABSENT)).expect_err("absent id refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_FOUND", "{error:?}");
}

#[test]
fn must_fail_label_export_of_state_only_routine_is_not_mined_precondition() {
    let (_dir, db) = db_with_state_only_row();
    let error =
        export_routine_label(&db, &label_params(STATE_ONLY)).expect_err("state-only id refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_MINED", "{error:?}");
}

#[test]
fn must_fail_compile_plan_of_absent_routine_is_not_found() {
    let (_dir, db) = db_with_state_only_row();
    let error = compile_routine_plan(&db, &plan_params(ABSENT)).expect_err("absent id refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_FOUND", "{error:?}");
}

#[test]
fn must_fail_compile_plan_of_state_only_routine_is_not_mined_precondition() {
    let (_dir, db) = db_with_state_only_row();
    let error =
        compile_routine_plan(&db, &plan_params(STATE_ONLY)).expect_err("state-only id refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_MINED", "{error:?}");
}

#[test]
fn must_fail_malformed_id_stays_params_invalid_on_both_paths() {
    let (_dir, db) = db_with_state_only_row();
    let label = export_routine_label(&db, &label_params("rt1-NOPE")).expect_err("malformed");
    assert_eq!(code_of(&label), "TOOL_PARAMS_INVALID", "{label:?}");
    let plan = compile_routine_plan(&db, &plan_params("rt1-NOPE")).expect_err("malformed");
    assert_eq!(code_of(&plan), "TOOL_PARAMS_INVALID", "{plan:?}");
}

/// A temp profile dir holding one valid profile, so `routine automate` gets
/// past profile resolution and reaches the routine lookup.
fn automate_runtimes(
    db: &Arc<Db>,
) -> (
    tempfile::TempDir,
    synapse_profiles::ProfileRuntime,
    Arc<std::sync::Mutex<synapse_reflex::ReflexRuntime>>,
) {
    let profiles = tempfile::tempdir().expect("profile tempdir");
    std::fs::write(
        profiles.path().join("profile-98.toml"),
        r#"
id = "profile-98"
label = "Profile 98"
schema_version = 2
use_scope = "productivity"
mouse_velocity_profile_default = "natural"
keyboard_dynamics_default = "natural"

[[matches]]
exe = "profile98.exe"
"#,
    )
    .expect("write profile");
    let profile_runtime =
        synapse_profiles::ProfileRuntime::spawn(profiles.path()).expect("profile runtime");
    let (action, _rx) = synapse_action::ActionHandle::channel();
    let reflex_runtime = Arc::new(std::sync::Mutex::new(
        synapse_reflex::ReflexRuntime::spawn(
            Arc::clone(db),
            action,
            synapse_reflex::EventBus::default(),
        )
        .expect("reflex runtime"),
    ));
    (profiles, profile_runtime, reflex_runtime)
}

fn automate(id: &str) -> crate::m3::profile_authoring::RoutineAutomateParams {
    crate::m3::profile_authoring::RoutineAutomateParams {
        routine_id: id.to_owned(),
        profile_id: Some("profile-98".to_owned()),
        candidate_id: None,
        store_plan: true,
    }
}

fn assert_no_automation_row(db: &Db, id: &str) {
    let key = crate::m3::profile_authoring::routine_automation_key(id);
    let rows = db
        .scan_cf_prefix(cf::CF_KV, key.as_bytes())
        .expect("automation readback");
    assert!(
        rows.is_empty(),
        "a refused automate must not write a routine_automation row for {id}"
    );
}

#[test]
fn must_fail_automate_absent_routine_is_not_found() {
    let (_dir, db) = db_with_state_only_row();
    let (_profiles, profile_runtime, reflex_runtime) = automate_runtimes(&db);

    let absent = crate::m3::profile_authoring::generate_routine_automation_candidate(
        &profile_runtime,
        &reflex_runtime,
        &db,
        &automate(ABSENT),
    )
    .expect_err("absent id refuses");
    assert_eq!(code_of(&absent), "ROUTINE_NOT_FOUND", "{absent:?}");
    assert_no_automation_row(&db, ABSENT);

    let state_only = crate::m3::profile_authoring::generate_routine_automation_candidate(
        &profile_runtime,
        &reflex_runtime,
        &db,
        &automate(STATE_ONLY),
    )
    .expect_err("state-only id refuses");
    assert_eq!(code_of(&state_only), "ROUTINE_NOT_MINED", "{state_only:?}");
    assert_no_automation_row(&db, STATE_ONLY);
}

const MINED: &str = "rt1-00000000000000cc";

fn mined_routine(id: &str) -> RoutineRecord {
    RoutineRecord {
        record_version: ROUTINE_RECORD_VERSION,
        ts_ns: 100,
        routine_id: id.to_owned(),
        granularity: RoutineGranularity::App,
        steps: Vec::new(),
        dow_class: RoutineDowClass::Weekdays,
        mean_minute_of_day: 525,
        tolerance_minutes: 10,
        schedule_label: "weekdays".to_owned(),
        support_days: 5,
        occurrence_count: 5,
        opportunity_days: 5,
        confidence: 0.8,
        window_start_ns: 0,
        window_end_ns: 10,
        active_days_in_window: 5,
        first_seen_day_start_ns: 1,
        last_seen_day_start_ns: 9,
        evidence: Vec::new(),
    }
}

/// Seeds a mined `CF_ROUTINES` row for `id` next to the state-only row.
fn put_mined(db: &Arc<Db>, id: &str) {
    let key = routine_codec::routine_key(id).expect("routine key");
    db.put_batch_pressure_bypass(
        cf::CF_ROUTINES,
        [(key, encode_json(&mined_routine(id)).expect("encode"))],
    )
    .expect("put routine row");
}

#[test]
fn must_pass_label_export_of_mined_routine_returns_its_template() {
    let (_dir, db) = db_with_state_only_row();
    put_mined(&db, MINED);
    let bundle = export_routine_label(&db, &label_params(MINED)).expect("mined routine exports");
    assert_eq!(bundle.routine_id, MINED);
    assert!(!bundle.state_row_exists);
    assert_eq!(bundle.lifecycle, RoutineLifecycle::Candidate);
    assert_eq!(bundle.current_label, None);
    assert_eq!(bundle.schedule_label, "weekdays");
    assert_eq!(bundle.mean_minute_of_day, 525);
    assert_eq!(bundle.support_days, 5);
    assert!(bundle.steps.is_empty());
    assert!(bundle.samples.is_empty());
    assert!(
        bundle.writeback_hint.contains(MINED),
        "{}",
        bundle.writeback_hint
    );
}

#[test]
fn must_pass_label_export_of_mined_routine_with_state_row_reports_its_label() {
    let (_dir, db) = db_with_state_only_row();
    put_mined(&db, STATE_ONLY);
    let bundle =
        export_routine_label(&db, &label_params(STATE_ONLY)).expect("mined routine exports");
    assert!(bundle.state_row_exists);
    assert_eq!(bundle.lifecycle, RoutineLifecycle::Confirmed);
    assert_eq!(bundle.current_label.as_deref(), Some("operator label"));
    assert!(
        bundle.prompt.contains("Current label: operator label"),
        "{}",
        bundle.prompt
    );
}

#[test]
fn must_pass_compile_plan_of_mined_routine_stores_and_reads_back() {
    let (_dir, db) = db_with_state_only_row();
    put_mined(&db, MINED);
    let response = compile_routine_plan(
        &db,
        &RoutineCompilePlanParams {
            routine_id: MINED.to_owned(),
            store: true,
        },
    )
    .expect("mined routine compiles");
    assert!(response.stored);
    assert_eq!(response.plan.routine_id, MINED);
    assert_eq!(response.plan.schedule_label, "weekdays");
    assert_eq!(response.plan.total_steps, 0);
    let stored = crate::m3::plan::load_plan(&db, MINED)
        .expect("plan readback")
        .expect("plan persisted");
    assert_eq!(stored, response.plan);
}

#[path = "routine_taxonomy_98.rs"]
mod taxonomy_98;

#[cfg(test)]
#[path = "legacy_candidate_migration_174.rs"]
mod legacy_candidate_migration_174;

#[path = "routine_automation_candidate_digest_174.rs"]
mod routine_automation_candidate_digest_174;

#[path = "routine_pressure_shed_270.rs"]
mod routine_pressure_shed_270;
