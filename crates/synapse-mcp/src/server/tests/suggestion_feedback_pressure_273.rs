//! criteria_b2.md:3. Execute through suggestion_accept, then raise pressure
//! immediately before its feedback write. Never substitute finalization helpers.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use synapse_storage::{Db, cf};

const ID: &str = "sg1-00000000000000000000000000000273";
const ROUTINE: &str = "rt1-0000000000000273";
static PRESSURE_AT_FEEDBACK: Mutex<Option<std::sync::Weak<Db>>> = Mutex::new(None);

pub(super) fn raise_pressure_before_feedback(db: &Arc<Db>, accepted: &SuggestionRecord) {
    if accepted.suggestion_id != ID {
        return;
    }
    let mut slot = PRESSURE_AT_FEEDBACK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The DB identity makes concurrent services immune to this fault.
    if !slot
        .as_ref()
        .and_then(std::sync::Weak::upgrade)
        .is_some_and(|armed| Arc::ptr_eq(&armed, db))
    {
        return;
    }
    slot.take();
    let report = db
        .run_pressure_check_with_free_bytes_sample(0)
        .unwrap_or_else(|error| panic!("pressure prerequisite: {error}"));
    assert_eq!(
        report.current_level,
        synapse_storage::DiskPressureLevel::Level4
    );
}

struct ResetPressure;
impl Drop for ResetPressure {
    fn drop(&mut self) {
        PRESSURE_AT_FEEDBACK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

#[tokio::test]
async fn must_fail_suggestion_accept_feedback_shed_is_nonretryable_and_registered_273()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let service = crate::test_support::service_in(dir.path())?;
    let db = service.m3_storage()?;
    let suggestion = json!({
        "record_version":1,"suggestion_id":ID,"routine_id":ROUTINE,"source":"routine_intent",
        "created_ts_ns":1,"expiry_ts_ns":u64::MAX/2,"status":"live","confidence":0.9,
        "matched_prefix_len":0,"total_steps":1,"remaining_step_count":1
    });
    let plan = json!({
        "record_version":1,"routine_id":ROUTINE,"compiled_ts_ns":1,"granularity":"app",
        "schedule_label":"batch2","total_steps":1,"deterministic_steps":0,
        "agent_task_steps":1,"fully_deterministic":false,
        "steps":[{"index":0,"source_app":"fixture","backend":"agent_task","deterministic":false,
            "action":"fixture refusal","postcondition":{"kind":"agent_reported"},
            "agent_task_reason":"deterministic unsupported step"}]
    });
    // Deserialize before seeding: invalid fixture data must never become RED.
    let _: SuggestionRecord = serde_json::from_value(suggestion.clone())?;
    let _: PlanDocument = serde_json::from_value(plan.clone())?;
    let state = json!({
        "record_version":2,"routine_id":ROUTINE,"lifecycle":"confirmed",
        "created_ts_ns":1,"updated_ts_ns":1,"present_in_last_mine":true,
        "transitions":[],"transitions_truncated":0,"confidence_history":[],
        "confidence_history_truncated":0
    });
    let _: synapse_core::types::RoutineStateRecord = serde_json::from_value(state.clone())?;
    // The loader follows a hashed ID index, not suggestion/v1/<suggestion_id>.
    let primary_key = format!("suggestion/v1/{ROUTINE}/{:020}", 1);
    let primary_value = serde_json::to_vec(&suggestion)?;
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let index_key = format!("suggestion_id/v1/{}/primary", hex(ID.as_bytes()));
    let index = json!({
        "record_version":1,"suggestion_id":ID,"routine_id":ROUTINE,"created_ts_ns":1,
        "primary_key_hex":hex(primary_key.as_bytes()),
        "primary_value_sha256":hex(Sha256::digest(&primary_value).as_slice())
    });
    db.put_batch(
        cf::CF_ROUTINE_STATE,
        [(ROUTINE.as_bytes(), serde_json::to_vec(&state)?)],
    )?;
    db.put_batch(
        cf::CF_KV,
        [
            (primary_key.into_bytes(), primary_value),
            (index_key.into_bytes(), serde_json::to_vec(&index)?),
            (
                format!("plan/v1/{ROUTINE}").into_bytes(),
                serde_json::to_vec(&plan)?,
            ),
        ],
    )?;
    db.flush()?;
    assert_eq!(
        load_suggestion_by_id(&db, ID)?
            .ok_or_else(|| anyhow::anyhow!("seed"))?
            .status,
        crate::m3::suggestions::SuggestionStatus::Live
    );
    assert!(
        db.scan_cf_prefix(cf::CF_KV, b"plan_execution/v1/")?
            .is_empty()
    );
    let feedback_before = db.scan_cf(cf::CF_ROUTINE_STATE)?;
    let _reset = ResetPressure;
    *PRESSURE_AT_FEEDBACK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&db));
    let outcome = service
        .suggestion_accept(
            Parameters(serde_json::from_value(json!({
                "suggestion_id":ID,"dry_run":false,"launch_timeout_ms":1000
            }))?),
            crate::test_support::request_context_for_session("batch2-suggestion").await,
        )
        .await;
    assert!(
        PRESSURE_AT_FEEDBACK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none(),
        "handler never reached feedback boundary: {:?}",
        outcome.as_ref().err()
    );
    let rows = db.scan_cf_prefix(cf::CF_KV, b"plan_execution/v1/")?;
    assert_eq!(
        rows.len(),
        1,
        "executed report must persist: {:?}",
        outcome.as_ref().err()
    );
    let execution: Value = serde_json::from_slice(&rows[0].1)?;
    assert_eq!(
        execution["steps"].as_array().map(Vec::len),
        Some(1),
        "real step must run: {execution}"
    );
    assert_ne!(
        load_suggestion_by_id(&db, ID)?
            .ok_or_else(|| anyhow::anyhow!("accepted row"))?
            .status,
        crate::m3::suggestions::SuggestionStatus::Live
    );
    let error = outcome
        .err()
        .ok_or_else(|| anyhow::anyhow!("feedback shed silently acknowledged"))?;
    assert_eq!(
        db.scan_cf(cf::CF_ROUTINE_STATE)?,
        feedback_before,
        "shed feedback changed state"
    );
    let data = error
        .data
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("typed error"))?;
    assert_ne!(data["code"], error_codes::STORAGE_WRITE_SHED);
    assert_eq!(data["retryable"], false);
    assert_eq!(data["code"], "EXECUTION_FINALIZATION_FAILED");
    // Adjunct registration oracle, after exercising the real handler above.
    assert!(
        include_str!("../../../../synapse-core/src/error_codes.rs")
            .contains("pub const EXECUTION_FINALIZATION_FAILED:"),
        "returned code missing from registry"
    );
    Ok(())
}
