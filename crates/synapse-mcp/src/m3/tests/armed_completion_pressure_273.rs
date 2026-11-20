//! #273: pressure rising after a durable claim must not strand Started rows.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::m3::tests::pressure_shed_support_270::force_pressure;

fn claimed_run(db: &Arc<Db>) -> ArmedRoutineRunRecord {
    install_automation(db, MINED, "plan/273");
    arm_routine(db, MINED, arm_config(), "test-273", None).expect("arm before pressure");
    let due_ts_ns = 1_778_803_200_000_000_000;
    let due = ArmedRoutineDueRun {
        routine_id: MINED.to_owned(),
        trigger_kind: ArmedRoutineTriggerKind::Schedule,
        trigger_key: "schedule:273".to_owned(),
        due_ts_ns,
        plan_ref: Some("plan/273".to_owned()),
        intent: None,
    };
    let run = claim_armed_run(db, &due, due_ts_ns + 1).expect("claim before pressure");
    assert_eq!(
        load_armed_run(db, &run.run_id)
            .expect("read claim")
            .expect("row")
            .status,
        ArmedRoutineRunStatus::Started
    );
    run
}

// This is the real completion call used by execute_claimed_armed_run. Synthetic
// step evidence is input to that call, not a claim that a desktop action ran.
fn finish(db: &Arc<Db>, run: ArmedRoutineRunRecord) -> Result<ArmedRoutineRunRecord, ErrorData> {
    complete_armed_run(
        db,
        run,
        ArmedRoutineRunStatus::Failed,
        Some("plan/273".into()),
        Some("exec/273".into()),
        None,
        Some("AUDIT_WRITE_FAILED".into()),
        Some("pressure rose after execution".into()),
        serde_json::json!({"succeeded_steps": 1, "completed_actions": ["synthetic-step-273"]}),
    )
}

#[test]
fn must_pass_started_run_finalizes_after_pressure_rises_273() {
    let (_dir, db) = test_db();
    let run = claimed_run(&db);
    let run_id = run.run_id.clone();
    force_pressure(&db);
    let outcome = finish(&db, run);
    let stored = load_armed_run(&db, &run_id)
        .expect("independent completion read")
        .expect("row");
    assert_ne!(
        stored.status,
        ArmedRoutineRunStatus::Started,
        "#273: completion stranded the durable claim: {outcome:?}"
    );
    assert_eq!(stored.status, ArmedRoutineRunStatus::Failed);
    assert!(stored.completed_ts_ns >= stored.started_ts_ns);
    let armed = load_armed_routine_record(&db, MINED)
        .expect("armed read")
        .expect("armed row");
    assert_eq!(armed.last_run_status, Some(ArmedRoutineRunStatus::Failed));
}

#[test]
fn must_pass_completed_action_evidence_survives_pressure_273() {
    let (_dir, db) = test_db();
    let run = claimed_run(&db);
    let run_id = run.run_id.clone();
    force_pressure(&db);
    let outcome = finish(&db, run);
    let stored = load_armed_run(&db, &run_id)
        .expect("independent evidence read")
        .expect("row");
    assert_eq!(
        stored.evidence["succeeded_steps"], 1,
        "#273: already-completed action evidence lost: {outcome:?}"
    );
    assert_eq!(
        stored.evidence["completed_actions"],
        serde_json::json!(["synthetic-step-273"])
    );
    if let Err(error) = outcome {
        assert_ne!(
            code_of(&error),
            error_codes::STORAGE_WRITE_SHED,
            "post-action failure must not invite replay"
        );
    }
}
