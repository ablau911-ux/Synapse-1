//! #174: exercise generation, including the ID assigned before lazy re-keying.
use super::*;
use crate::m3::profile_authoring::{
    ProfileAuthoringCandidate, generate_routine_automation_candidate,
};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn must_fail_routine_automation_candidate_cannot_expose_plain_content_digest_174() {
    let (_dir, db) = db_with_state_only_row();
    put_mined(&db, MINED);
    let (_profiles, profile, runtime) = automate_runtimes(&db);
    let prefix = b"profile_authoring/v1/candidate/";
    assert!(
        db.scan_cf_prefix(cf::CF_PROFILES, prefix)
            .expect("before candidates")
            .is_empty()
    );
    assert_no_automation_row(&db, MINED);

    let generated =
        generate_routine_automation_candidate(&profile, &runtime, &db, &automate(MINED))
            .expect("generate real routine automation candidate");
    let routine = mined_routine(MINED);
    let plan = &generated.plan;
    // Independent expected payload: never trust a possibly reverted evidence helper.
    let payload = serde_json::json!({
        "kind": "routine_automation", "profile_id": "profile-98", "routine": routine,
        "plan": {
            "record_version": plan.record_version, "routine_id": &plan.routine_id,
            "granularity": &plan.granularity, "schedule_label": &plan.schedule_label,
            "total_steps": plan.total_steps, "deterministic_steps": plan.deterministic_steps,
            "agent_task_steps": plan.agent_task_steps, "fully_deterministic": plan.fully_deterministic,
            "steps": &plan.steps,
        },
    });
    let bytes = serde_json::to_vec(&payload).expect("expected evidence bytes");
    let keyed = crate::content_digest::keyed_prefixed_digest(&bytes);
    let keyed_hex = keyed
        .strip_prefix("hmac-sha256:")
        .expect("keyed digest marker");
    let plain = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_ne!(
        &keyed_hex[..16],
        &plain[..16],
        "keyed expectation must differ from plain SHA-256"
    );
    let expected_id = format!("routine-auto.{MINED}.{}", &keyed_hex[..16]);
    assert_eq!(
        generated.candidate.candidate_id, expected_id,
        "candidate ID must be keyed before encode_candidate can mask a reverted hash helper"
    );
    assert_ne!(
        generated.candidate.candidate_id,
        format!("routine-auto.{MINED}.{}", &plain[..16])
    );
    assert_eq!(generated.candidate.source.evidence_hash, keyed);

    let rows = db
        .scan_cf_prefix(cf::CF_PROFILES, prefix)
        .expect("separate persisted readback");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, generated.row_key.as_bytes());
    let stored: ProfileAuthoringCandidate =
        synapse_storage::decode_json(&rows[0].1).expect("candidate bytes");
    assert!(stored.source.evidence_hash.starts_with("hmac-sha256:"));
    assert_eq!(stored.source.evidence_hash, keyed);
    assert_eq!(stored.candidate_id, expected_id);
    let automation = crate::m3::profile_authoring::load_routine_automation_record(&db, MINED)
        .expect("automation readback")
        .expect("automation persisted");
    assert_eq!(automation.candidate_id, expected_id);
    assert_eq!(automation.candidate_row_key, generated.row_key);
}
