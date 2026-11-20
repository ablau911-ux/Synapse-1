//! #270: every armed-routine pressure gate (arm, disarm, tick claim, and the
//! shared armed+run record writer) refuses with `STORAGE_WRITE_SHED` like
//! `routine_mine` (#165), same data shape, nothing written. Child of
//! routine_arming_missing_98_tests: reuses its db fixture and armed-row check.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

use crate::m3::tests::pressure_shed_support_270::{assert_shed, force_pressure, snapshot};

fn pressured_db() -> (tempfile::TempDir, Arc<Db>) {
    let (dir, db) = test_db();
    force_pressure(&db);
    (dir, db)
}

#[test]
fn must_fail_arm_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db();
    let before = snapshot(&db);
    let error = arm_routine(&db, MINED, arm_config(), "s270", None)
        .expect_err("pressure must refuse arming");
    assert_shed(&error, "routine_update action=arm");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
    assert_no_armed_row(&db, MINED);
}

#[test]
fn must_fail_disarm_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db();
    let before = snapshot(&db);
    let error = disarm_routine(&db, MINED, "s270", Some("operator stop".to_owned()))
        .expect_err("pressure must refuse disarming");
    assert_shed(&error, "routine_update action=disarm");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
}

#[test]
fn must_fail_armed_tick_claim_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db();
    let before = snapshot(&db);
    let due = ArmedRoutineDueRun {
        routine_id: MINED.to_owned(),
        trigger_kind: ArmedRoutineTriggerKind::Schedule,
        trigger_key: "schedule:270".to_owned(),
        due_ts_ns: 1,
        plan_ref: None,
        intent: None,
    };
    let error = claim_armed_run(&db, &due, 2).expect_err("pressure must refuse the claim");
    assert_shed(&error, "armed_routine_tick");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
}

fn armed_record() -> ArmedRoutineRecord {
    ArmedRoutineRecord {
        record_version: ARMED_ROUTINE_RECORD_VERSION,
        row_kind: "armed_routine".to_owned(),
        routine_id: MINED.to_owned(),
        enabled: true,
        schedule_enabled: true,
        intent_enabled: false,
        failure_threshold: DEFAULT_FAILURE_THRESHOLD,
        consecutive_failures: 0,
        created_at_ns: 1,
        updated_at_ns: 1,
        armed_at_ns: Some(1),
        armed_by: Some("s270".to_owned()),
        arm_note: None,
        disarmed_at_ns: None,
        disarmed_by: None,
        disarm_reason: None,
        plan_ref: None,
        last_schedule_fire_key: None,
        last_intent_fire_key: None,
        last_run_id: None,
        last_run_status: None,
        source_of_truth: ARMED_ROUTINE_SOURCE_OF_TRUTH.to_owned(),
    }
}

fn run_record() -> ArmedRoutineRunRecord {
    ArmedRoutineRunRecord {
        record_version: ARMED_ROUTINE_RUN_RECORD_VERSION,
        row_kind: "armed_routine_run".to_owned(),
        run_id: "run-270".to_owned(),
        routine_id: MINED.to_owned(),
        trigger_kind: ArmedRoutineTriggerKind::Schedule,
        trigger_key: "schedule:270".to_owned(),
        started_ts_ns: 1,
        completed_ts_ns: 1,
        dry_run: false,
        status: ArmedRoutineRunStatus::Started,
        plan_ref: None,
        execution_id: None,
        approval_id: None,
        failure_count_after: 0,
        disarmed_after_failure: false,
        intent: None,
        error_code: None,
        error: None,
        evidence: serde_json::Value::Null,
        source_of_truth: ARMED_ROUTINE_SOURCE_OF_TRUTH.to_owned(),
    }
}

/// The record writer is the second gate on the claim/complete paths; a
/// pressure change between the first gate and the write must also shed.
#[test]
fn must_fail_armed_and_run_record_write_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db();
    let before = snapshot(&db);
    let error = write_armed_and_run_records(&db, &armed_record(), &run_record())
        .expect_err("pressure must refuse the armed+run record write");
    assert_shed(&error, "armed routine write");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
}
