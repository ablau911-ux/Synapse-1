//! #174: public decision path upgrades evidence without moving references.
use super::*;
use crate::m3::profile_authoring::*;

const ID: &str = "routine-auto.rt1-aaaaaaaaaaaaaaaa.feedfacefeedface";
const ROUTINE: &str = "rt1-aaaaaaaaaaaaaaaa";

fn legacy(state: &str) -> ProfileAuthoringCandidate {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1, "row_kind": "profile_authoring_candidate", "candidate_id": ID,
        "profile_id": "profile-98", "state": state, "generated_at_ns": 1, "updated_at_ns": 1,
        "accepted_at_ns": null, "rejected_at_ns": null, "operator_note": null, "rejection_reason": null,
        "source": {"audit_cf_name": "audit", "profile_cf_name": "profiles", "replay_path": null,
            "audit_rows_scanned": 1, "audit_rows_relevant": 1, "replay_rows_scanned": 0,
            "replay_rows_relevant": 0, "source_row_keys": ["private-row"],
            "source_audit_ids": ["audit174"], "evidence_hash": format!("sha256:{}", "a".repeat(64))},
        "evidence_summary": {"private": "title"}, "expected_improvement": [],
        "patch": {"routine_automation": {"routine_id": ROUTINE, "candidate_id": ID,
            "plan_ref": "plan/v1/original-174"}},
        "safety_review": {"activation": {"required": true, "active_on_accept": false},
            "contribution": {"registry_allowed": false, "external_sharing_allowed": false},
            "unsafe_permission_escalation": false, "rejected_reasons": []}
    })).unwrap()
}

fn seed(db: &Db, state: &str) -> (String, Vec<u8>) {
    let key = format!("profile_authoring/v1/candidate/{ID}");
    let bytes = synapse_storage::encode_json(&legacy(state)).unwrap();
    db.put_batch_pressure_bypass(cf::CF_PROFILES, [(key.as_bytes().to_vec(), bytes.clone())])
        .unwrap();
    assert_eq!(
        db.get_cf_exact(cf::CF_PROFILES, key.as_bytes()).unwrap(),
        Some(bytes.clone())
    );
    (key, bytes)
}

fn reject() -> ProfileAuthoringDecideParams {
    ProfileAuthoringDecideParams {
        candidate_id: ID.into(),
        decision: ProfileAuthoringDecision::Reject,
        operator_note: None,
        reason: Some("operator declined".into()),
    }
}

fn reference(db: &Arc<Db>) {
    let row = RoutineAutomationRecord {
        schema_version: 1,
        row_kind: "routine_automation".into(),
        routine_id: ROUTINE.into(),
        profile_id: "profile-98".into(),
        candidate_id: ID.into(),
        candidate_row_key: format!("profile_authoring/v1/candidate/{ID}"),
        plan_ref: "plan/v1/original-174".into(),
        state: "candidate".into(),
        generated_at_ns: 1,
        updated_at_ns: 1,
        installed_at_ns: None,
        rejected_at_ns: None,
        plan_fully_deterministic: true,
        total_steps: 2,
        deterministic_steps: 2,
        agent_task_steps: 0,
    };
    db.put_batch_pressure_bypass(
        cf::CF_KV,
        [(
            format!("routine_automation/v1/{ROUTINE}").into_bytes(),
            synapse_storage::encode_json(&row).unwrap(),
        )],
    )
    .unwrap();
    assert_eq!(
        load_routine_automation_record(db, ROUTINE)
            .unwrap()
            .unwrap()
            .candidate_id,
        ID
    );
}

#[tokio::test]
async fn must_pass_legacy_rejection_rekeys_evidence_only_preserves_references_174() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), synapse_core::SCHEMA_VERSION).unwrap());
    let (_profiles, profile, runtime) = automate_runtimes(&db);
    let (key, before) = seed(&db, "candidate");
    reference(&db);
    let inspected = inspect_profile_authoring_candidate(
        &runtime,
        &ProfileAuthoringInspectParams {
            candidate_id: ID.into(),
        },
    )
    .unwrap();
    assert_eq!(
        inspected.candidate.unwrap().source.evidence_hash,
        legacy("candidate").source.evidence_hash
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_PROFILES, key.as_bytes()).unwrap(),
        Some(before)
    );
    let response = decide_profile_authoring_candidate(&profile, &runtime, &db, &reject()).unwrap();
    assert_eq!(response.row_key, key);
    assert_eq!(response.candidate_id, ID);
    assert_eq!(response.state, "rejected");
    let stored: ProfileAuthoringCandidate = synapse_storage::decode_json(
        &db.get_cf_exact(cf::CF_PROFILES, key.as_bytes())
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(stored.source.evidence_hash.get(..12), Some("hmac-sha256:"));
    assert_eq!(stored.source.evidence_hash.len(), 76);
    assert_eq!(stored.candidate_id, ID);
    assert_eq!(stored.patch, legacy("candidate").patch);
    assert_eq!(stored.source.source_row_keys, vec!["private-row"]);
    let reference = load_routine_automation_record(&db, ROUTINE)
        .unwrap()
        .unwrap();
    assert_eq!(reference.candidate_id, ID);
    assert_eq!(reference.candidate_row_key, key);
    assert_eq!(reference.plan_ref, "plan/v1/original-174");
    assert_eq!(reference.state, "rejected");
    assert_eq!(
        db.scan_cf_prefix(cf::CF_PROFILES, b"profile_authoring/v1/")
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn must_fail_invalid_decision_cannot_migrate_or_move_legacy_candidate_174() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), synapse_core::SCHEMA_VERSION).unwrap());
    let (_profiles, profile, runtime) = automate_runtimes(&db);
    let (key, before) = seed(&db, "candidate");
    let mut params = reject();
    params.operator_note = Some("invalid for reject".into());
    let error = decide_profile_authoring_candidate(&profile, &runtime, &db, &params).unwrap_err();
    assert_eq!(
        error.data.as_ref().unwrap()["code"],
        synapse_core::error_codes::TOOL_PARAMS_INVALID
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_PROFILES, key.as_bytes()).unwrap(),
        Some(before)
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_PROFILES, b"profile_authoring/v1/")
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn must_fail_accepted_legacy_candidate_cannot_be_rejected_or_rekeyed_174() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path(), synapse_core::SCHEMA_VERSION).unwrap());
    let (_profiles, profile, runtime) = automate_runtimes(&db);
    let (key, before) = seed(&db, "accepted");
    let error = decide_profile_authoring_candidate(&profile, &runtime, &db, &reject()).unwrap_err();
    assert_eq!(
        error.data.as_ref().unwrap()["code"],
        synapse_core::error_codes::PROFILE_AUTHORING_INVALID_STATE
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_PROFILES, key.as_bytes()).unwrap(),
        Some(before)
    );
}
