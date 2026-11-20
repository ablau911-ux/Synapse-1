//! #273: exercise the real server continuation after the claim/pressure race.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::m3::armed_routines::{ArmedRoutineDueRun, ArmedRoutineRecord, ArmedRoutineTriggerKind};
use synapse_storage::Db;

const ROUTINE: &str = "rt1-0000000000000273";

fn seed_claim(db: &std::sync::Arc<Db>) -> anyhow::Result<ArmedRoutineRunRecord> {
    let armed = ArmedRoutineRecord {
        record_version: 1,
        row_kind: "armed_routine".into(),
        routine_id: ROUTINE.into(),
        enabled: true,
        schedule_enabled: false,
        intent_enabled: true,
        failure_threshold: 3,
        consecutive_failures: 0,
        created_at_ns: 1,
        updated_at_ns: 1,
        armed_at_ns: Some(1),
        armed_by: Some("test-273".into()),
        arm_note: None,
        disarmed_at_ns: None,
        disarmed_by: None,
        disarm_reason: None,
        plan_ref: None,
        last_schedule_fire_key: None,
        last_intent_fire_key: None,
        last_run_id: None,
        last_run_status: None,
        source_of_truth: ARMED_ROUTINE_SOURCE_OF_TRUTH.into(),
    };
    db.put_batch_pressure_bypass(
        cf::CF_KV,
        [(
            format!("armed_routine/v1/{ROUTINE}").into_bytes(),
            serde_json::to_vec(&armed)?,
        )],
    )?;
    let run = claim_armed_run(
        db,
        &ArmedRoutineDueRun {
            routine_id: ROUTINE.into(),
            trigger_kind: ArmedRoutineTriggerKind::Intent,
            trigger_key: "intent:273".into(),
            due_ts_ns: 1,
            plan_ref: None,
            intent: None,
        },
        2,
    )?;
    let stored = read_run(db, &run.run_id)?;
    assert_eq!(stored.status, ArmedRoutineRunStatus::Started);
    Ok(run)
}

fn read_run(db: &Db, id: &str) -> anyhow::Result<ArmedRoutineRunRecord> {
    let bytes = db
        .get_cf_exact(cf::CF_KV, format!("armed_routine_run/v1/{id}").as_bytes())?
        .expect("claimed run persists");
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn must_pass_server_continuation_finalizes_claim_on_pressure_273() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let run = seed_claim(&db)?;
    let id = run.run_id.clone();
    // No routine plan exists: the real server's plan-error completion path
    // must close the already-written claim even when pressure rises here.
    let report = db.run_pressure_check_with_free_bytes_sample(0)?;
    assert_eq!(
        report.current_level,
        synapse_storage::DiskPressureLevel::Level4
    );
    assert!(!db.pressure_permits_write(cf::CF_KV));
    let outcome = svc
        .execute_claimed_armed_run(&db, &ArmedRoutineTickParams::default(), run, None)
        .await;
    let stored = read_run(&db, &id)?;
    assert_ne!(
        stored.status,
        ArmedRoutineRunStatus::Started,
        "real server path stranded a claim: {outcome:?}"
    );
    if let Err(error) = outcome {
        assert_ne!(
            code(&error),
            error_codes::STORAGE_WRITE_SHED,
            "a durable claim already exists; replay is unsafe"
        );
    }
    Ok(())
}
