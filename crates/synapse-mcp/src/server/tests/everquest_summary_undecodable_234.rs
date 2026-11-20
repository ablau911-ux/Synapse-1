//! #234: an undecodable reality head must block EverQuest movement as
//! severely as a missing one, and say why.

use super::*;

fn undecodable_context() -> EverQuestWorldSummaryRealityContext {
    missing_reality_context(
        "everquest".to_owned(),
        "reality/head/v1/everquest".to_owned(),
        true,
    )
}

#[test]
fn must_fail_undecodable_head_is_a_high_hazard_and_a_blocker() {
    // Real derivation: a garbage head row at reality/head/v1/everquest is read
    // by `summary_reality_context`, which must report it present-but-undecodable.
    let temp = tempfile::TempDir::new().expect("tempdir");
    let service = SynapseService::try_new_isolated_for_tests(temp.path()).expect("service");
    {
        let runtime = service.reflex_runtime().expect("runtime");
        let runtime = runtime.lock().expect("runtime lock");
        runtime
            .storage_put_kv_rows(vec![(
                b"reality/head/v1/everquest".to_vec(),
                b"{not a head row".to_vec(),
            )])
            .expect("plant undecodable head");
    }
    let params = NormalizedSummaryParams {
        summary_id: "i234-summary".to_owned(),
        profile_id: "everquest".to_owned(),
        state_row_key: "i234-state".to_owned(),
        state_override: None,
        install_root_override: None,
        max_exits: 4,
        max_landmarks: 4,
        max_transitions: 4,
        max_hazards: 4,
        stale_after_seconds: 60,
        source_refs: Vec::new(),
        row_key: "i234-row".to_owned(),
    };
    let context = service
        .summary_reality_context(&params)
        .expect("an undecodable head is reported, not an error");
    assert_eq!(context, undecodable_context());
    assert_eq!(context.status, "baseline_undecodable");
    assert!(context.head_present);
    let hazards = reality_hazards(&context);
    assert_eq!(hazards.len(), 1);
    assert_eq!(hazards[0].code, "reality_baseline_undecodable");
    assert_eq!(hazards[0].severity, "high");
    assert_eq!(
        reality_blockers(&context),
        vec!["reality_baseline_undecodable".to_owned()]
    );
    assert_eq!(
        reality_safe_next_probe(&context.status),
        "capture_reality_baseline"
    );
}

#[test]
fn must_pass_missing_head_keeps_its_own_hazard_and_blocker() {
    let context = missing_reality_context(
        "everquest".to_owned(),
        "reality/head/v1/everquest".to_owned(),
        false,
    );
    assert_eq!(context.status, "baseline_missing");
    assert!(!context.head_present);
    assert_eq!(
        reality_hazards(&context)[0].code,
        "reality_baseline_missing"
    );
    assert_eq!(
        reality_blockers(&context),
        vec!["reality_baseline_missing".to_owned()]
    );
}
