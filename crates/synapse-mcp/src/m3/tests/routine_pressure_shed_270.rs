//! #270: the routine tools' storage-pressure gates (routine_feedback,
//! routine_update, routine_compile_plan, routine_automate) refuse with
//! `STORAGE_WRITE_SHED` like `routine_mine` (#165), with the same data shape
//! and nothing written. Child of routine_missing_98_tests: reuses its fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

use crate::m3::tests::pressure_shed_support_270::{assert_shed, force_pressure, snapshot};

const TOOL_SESSION: &str = "s270";

fn pressured_db_with_mined_routine() -> (tempfile::TempDir, Arc<Db>) {
    let (dir, db) = db_with_state_only_row();
    put_mined(&db, MINED);
    force_pressure(&db);
    (dir, db)
}

/// Anchor: the reference gate itself. `routine_mine` sheds a real write and
/// lets a dry run through, so the shape the other gates must match is fixed.
#[test]
fn must_pass_mine_gate_is_the_reference_shed_shape_270() {
    let (_dir, db) = pressured_db_with_mined_routine();
    let error = check_mine_pressure(&db, false).expect_err("mine write is shed");
    assert_shed(&error, "routine_mine");
    check_mine_pressure(&db, true).expect("a dry run writes nothing, so it is admitted");
}

#[test]
fn must_fail_routine_feedback_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db_with_mined_routine();
    let before = snapshot(&db);
    let params: RoutineFeedbackParams = serde_json::from_value(serde_json::json!({
        "routine_id": MINED, "outcome": "accepted", "now_ts_ns": 1_u64
    }))
    .expect("feedback params");
    let error = record_routine_feedback(&db, &params, TOOL_SESSION)
        .expect_err("pressure must refuse the feedback write");
    assert_shed(&error, "routine_feedback");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
}

#[test]
fn must_fail_routine_update_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db_with_mined_routine();
    let before = snapshot(&db);
    let params: RoutineUpdateParams = serde_json::from_value(serde_json::json!({
        "routine_id": MINED, "action": "confirm"
    }))
    .expect("update params");
    let error = update_routine(&db, &params, TOOL_SESSION)
        .expect_err("pressure must refuse the lifecycle write");
    assert_shed(&error, "routine_update");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
}

#[test]
fn must_fail_routine_compile_plan_store_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db_with_mined_routine();
    let before = snapshot(&db);
    let error = compile_routine_plan(
        &db,
        &RoutineCompilePlanParams {
            routine_id: MINED.to_owned(),
            store: true,
        },
    )
    .expect_err("pressure must refuse storing the plan");
    assert_shed(&error, "routine_compile_plan");
    assert_eq!(snapshot(&db), before, "no plan row is written");
}

/// Guard: only the storing path is gated; compiling without `store` writes
/// nothing and must keep working under pressure.
#[test]
fn must_pass_routine_compile_plan_without_store_is_admitted_under_pressure_270() {
    let (_dir, db) = pressured_db_with_mined_routine();
    let before = snapshot(&db);
    let response = compile_routine_plan(
        &db,
        &RoutineCompilePlanParams {
            routine_id: MINED.to_owned(),
            store: false,
        },
    )
    .expect("a compile that stores nothing is not a write");
    assert!(!response.stored);
    assert_eq!(snapshot(&db), before);
}

#[test]
fn must_fail_routine_automate_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = db_with_state_only_row();
    put_mined(&db, MINED);
    // Runtimes first: spawning them is not part of the refused write.
    let (_profiles, profile_runtime, reflex_runtime) = automate_runtimes(&db);
    force_pressure(&db);
    let before = snapshot(&db);
    let error = crate::m3::profile_authoring::generate_routine_automation_candidate(
        &profile_runtime,
        &reflex_runtime,
        &db,
        &automate(MINED),
    )
    .expect_err("pressure must refuse the automation candidate write");
    assert_shed(&error, "routine_automate");
    assert_eq!(
        snapshot(&db),
        before,
        "no automation or plan row is written"
    );
    assert_no_automation_row(&db, MINED);
}

#[test]
fn must_fail_plan_execution_write_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_dir, db) = pressured_db_with_mined_routine();
    let before = snapshot(&db);
    let record = crate::m3::plan_execution::build_plan_execution_record(
        "px1-270".to_owned(),
        "sg1-00000000000000000000000000000270".to_owned(),
        None,
        1,
        1,
        2,
        false,
        crate::m3::plan::compile_plan(&mined_routine(MINED), 1),
        Vec::new(),
    );
    let error = crate::m3::plan_execution::write_plan_execution(&db, &record)
        .expect_err("pressure must refuse the plan execution write");
    assert_shed(&error, "plan execution write");
    assert_eq!(snapshot(&db), before, "no plan_execution row is written");
}
