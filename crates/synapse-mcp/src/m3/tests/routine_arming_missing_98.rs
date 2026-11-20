//! Regression tests for #98 (second re-review): `routine update action=arm`
//! and `action=disarm` on a missing routine must use the same domain codes as
//! label/plan/automate (`ROUTINE_NOT_FOUND` / `ROUTINE_NOT_MINED`), never
//! `TOOL_PARAMS_INVALID`. Disarming a mined routine that is simply not armed is
//! the distinct `ARMED_ROUTINE_NOT_FOUND`.

use super::*;

use synapse_core::SCHEMA_VERSION;
use synapse_core::types::{
    ROUTINE_RECORD_VERSION, ROUTINE_STATE_RECORD_VERSION, RoutineDowClass, RoutineGranularity,
    RoutineLifecycle, RoutineRecord, RoutineStateRecord,
};

const ABSENT: &str = "rt1-00000000000000aa";
const STATE_ONLY: &str = "rt1-00000000000000bb";
const MINED: &str = "rt1-00000000000000cc";

fn code_of(error: &ErrorData) -> String {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(|code| code.as_str())
        .unwrap_or_default()
        .to_owned()
}

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

fn test_db() -> (tempfile::TempDir, Arc<Db>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
    let state_key = routine_codec::routine_state_key(STATE_ONLY).expect("state key");
    db.put_batch_pressure_bypass(
        cf::CF_ROUTINE_STATE,
        [(
            state_key,
            encode_json(&state_row(STATE_ONLY)).expect("encode"),
        )],
    )
    .expect("put state row");
    let routine_key = routine_codec::routine_key(MINED).expect("routine key");
    db.put_batch_pressure_bypass(
        cf::CF_ROUTINES,
        [(
            routine_key,
            encode_json(&mined_routine(MINED)).expect("encode"),
        )],
    )
    .expect("put routine row");
    (dir, db)
}

fn arm_config() -> ArmRoutineConfig {
    ArmRoutineConfig::from_optional(None, None, None)
}

/// A refusal must leave no armed row behind: neither for `routine_id` nor
/// anywhere under the armed-routine prefix.
fn assert_no_armed_row(db: &Arc<Db>, routine_id: &str) {
    assert!(
        load_armed_routine_record(db, routine_id)
            .expect("armed readback")
            .is_none(),
        "a refused arm/disarm must not persist an armed row for {routine_id}"
    );
    let armed_rows = db
        .scan_cf_prefix(cf::CF_KV, ARMED_ROUTINE_PREFIX.as_bytes())
        .expect("armed prefix scan");
    assert!(
        armed_rows.is_empty(),
        "no armed row may exist after a refusal: {} rows",
        armed_rows.len()
    );
}

#[test]
fn must_fail_arm_absent_routine_is_not_found() {
    let (_dir, db) = test_db();
    let error = arm_routine(&db, ABSENT, arm_config(), "s1", None).expect_err("absent refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_FOUND", "{error:?}");
    assert_no_armed_row(&db, ABSENT);
}

#[test]
fn must_fail_arm_state_only_routine_is_not_mined() {
    let (_dir, db) = test_db();
    let error = arm_routine(&db, STATE_ONLY, arm_config(), "s1", None).expect_err("refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_MINED", "{error:?}");
    assert_no_armed_row(&db, STATE_ONLY);
}

#[test]
fn must_fail_disarm_absent_routine_is_not_found() {
    let (_dir, db) = test_db();
    let error = disarm_routine(&db, ABSENT, "s1", None).expect_err("absent refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_FOUND", "{error:?}");
    assert_no_armed_row(&db, ABSENT);
}

#[test]
fn must_fail_disarm_state_only_routine_is_not_mined() {
    let (_dir, db) = test_db();
    let error = disarm_routine(&db, STATE_ONLY, "s1", None).expect_err("refuses");
    assert_eq!(code_of(&error), "ROUTINE_NOT_MINED", "{error:?}");
    assert_no_armed_row(&db, STATE_ONLY);
}

#[test]
fn must_fail_disarm_mined_but_not_armed_routine_is_armed_routine_not_found() {
    let (_dir, db) = test_db();
    let error = disarm_routine(&db, MINED, "s1", None).expect_err("not armed refuses");
    assert_eq!(code_of(&error), "ARMED_ROUTINE_NOT_FOUND", "{error:?}");
    // Disarm must not create a disabled row for a routine that was never armed.
    assert_no_armed_row(&db, MINED);
}

#[test]
fn must_fail_malformed_id_stays_params_invalid_for_arm_and_disarm() {
    let (_dir, db) = test_db();
    let arm = arm_routine(&db, "rt1-NOPE", arm_config(), "s1", None).expect_err("malformed");
    assert_eq!(code_of(&arm), "TOOL_PARAMS_INVALID", "{arm:?}");
    let disarm = disarm_routine(&db, "rt1-NOPE", "s1", None).expect_err("malformed");
    assert_eq!(code_of(&disarm), "TOOL_PARAMS_INVALID", "{disarm:?}");
    let armed_rows = db
        .scan_cf_prefix(cf::CF_KV, ARMED_ROUTINE_PREFIX.as_bytes())
        .expect("armed prefix scan");
    assert!(armed_rows.is_empty(), "{} armed rows", armed_rows.len());
}

#[test]
fn must_fail_arm_mined_routine_without_installed_automation_is_refused() {
    // No automation row at all.
    let (_dir, db) = test_db();
    let error = arm_routine(&db, MINED, arm_config(), "s1", None).expect_err("no automation");
    assert_eq!(code_of(&error), "TOOL_PARAMS_INVALID", "{error:?}");
    assert!(
        error
            .message
            .starts_with("ROUTINE_AUTOMATION_NOT_INSTALLED"),
        "{error:?}"
    );
    assert_no_armed_row(&db, MINED);

    // An automation row with an empty plan_ref is not installed either.
    let (_dir, db) = test_db();
    install_automation(&db, MINED, "  ");
    let error = arm_routine(&db, MINED, arm_config(), "s1", None).expect_err("empty plan_ref");
    assert_eq!(code_of(&error), "TOOL_PARAMS_INVALID", "{error:?}");
    assert!(
        error
            .message
            .starts_with("ROUTINE_AUTOMATION_NOT_INSTALLED"),
        "{error:?}"
    );
    assert_no_armed_row(&db, MINED);
}

/// Seed an installed `routine_automation` row so `MINED` may be armed.
fn install_automation(db: &Arc<Db>, id: &str, plan_ref: &str) {
    let record = crate::m3::profile_authoring::RoutineAutomationRecord {
        schema_version: 1,
        row_kind: "routine_automation".to_owned(),
        routine_id: id.to_owned(),
        profile_id: "profile-98".to_owned(),
        candidate_id: "candidate-98".to_owned(),
        candidate_row_key: "candidate/98".to_owned(),
        plan_ref: plan_ref.to_owned(),
        state: "installed".to_owned(),
        generated_at_ns: 1,
        updated_at_ns: 1,
        installed_at_ns: Some(1),
        rejected_at_ns: None,
        plan_fully_deterministic: true,
        total_steps: 0,
        deterministic_steps: 0,
        agent_task_steps: 0,
    };
    let key = crate::m3::profile_authoring::routine_automation_key(id);
    db.put_batch_pressure_bypass(
        cf::CF_KV,
        [(key.into_bytes(), encode_json(&record).expect("encode"))],
    )
    .expect("put automation row");
}

#[test]
fn must_pass_arm_mined_routine_with_installed_automation_is_armed() {
    let (_dir, db) = test_db();
    install_automation(&db, MINED, "plan/98");
    let armed = arm_routine(&db, MINED, arm_config(), "s1", None).expect("mined routine arms");
    assert_eq!(armed.routine_id, MINED);
    assert!(armed.enabled);
    assert_eq!(armed.plan_ref.as_deref(), Some("plan/98"));
    assert_eq!(armed.armed_by.as_deref(), Some("s1"));
    let stored = load_armed_routine_record(&db, MINED)
        .expect("readback")
        .expect("armed row persisted");
    assert!(stored.enabled);
    assert_eq!(stored.plan_ref.as_deref(), Some("plan/98"));
}

#[test]
fn must_pass_disarm_armed_routine_persists_disabled_row() {
    let (_dir, db) = test_db();
    install_automation(&db, MINED, "plan/98");
    arm_routine(&db, MINED, arm_config(), "s1", None).expect("mined routine arms");
    let disarmed = disarm_routine(&db, MINED, "s2", Some("operator stop".to_owned()))
        .expect("armed routine disarms");
    assert!(!disarmed.enabled);
    assert_eq!(disarmed.disarmed_by.as_deref(), Some("s2"));
    assert_eq!(disarmed.disarm_reason.as_deref(), Some("operator stop"));
    let stored = load_armed_routine_record(&db, MINED)
        .expect("readback")
        .expect("armed row kept");
    assert!(!stored.enabled);
    assert!(stored.disarmed_at_ns.is_some());
}

#[path = "armed_pressure_shed_270.rs"]
mod armed_pressure_shed_270;

#[path = "armed_completion_pressure_273.rs"]
mod armed_completion_pressure_273;
