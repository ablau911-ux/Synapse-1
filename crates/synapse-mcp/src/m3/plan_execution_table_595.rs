//! FROZEN manager table C1-C14. No clock, storage, window, or OS dependency.
use super::*;
use PlanExecutionStatus as E;
use PlanStepExecutionStatus as S;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn report_only_control() {
    let (status, code, error) = AssistReadbackOutcome::new(true, true).into_parts();
    assert_eq!(
        status,
        S::Skipped,
        "CONTROL: a readback is not a verified correction"
    );
    assert_eq!(code, Some("ASSIST_CORRECTION_REPORT_ONLY"));
    assert!(error.is_some());
    assert_eq!(
        PlanExecutionOutcome::new(false, []).status(),
        E::Skipped,
        "CONTROL: empty plan cannot succeed"
    );
}

#[test]
fn c1_matching_readback_is_report_only() {
    report_only_control();
    assert_eq!(
        AssistReadbackOutcome::new(false, true).into_parts().0,
        S::Failed
    );
}

#[test]
fn c2_wrong_process_fails() {
    let (status, code, _) = AssistReadbackOutcome::new(false, true).into_parts();
    assert_eq!(status, S::Failed);
    assert_eq!(code, Some("ASSIST_TARGET_READBACK_MISMATCH"));
    report_only_control();
}

#[test]
fn c3_reused_or_other_pid_fails() {
    assert_eq!(
        AssistReadbackOutcome::new(true, false).into_parts().0,
        S::Failed
    );
    assert_eq!(
        AssistReadbackOutcome::new(false, false).into_parts().0,
        S::Failed
    );
    report_only_control();
}

#[test]
fn c4_only_skipped_steps_roll_up_skipped() {
    assert_eq!(
        PlanExecutionOutcome::new(false, [S::Skipped, S::Skipped]).status(),
        E::Skipped
    );
    report_only_control();
}

#[test]
fn c5_empty_plan_is_skipped() {
    assert_eq!(PlanExecutionOutcome::new(false, []).status(), E::Skipped);
    report_only_control();
}

#[test]
fn c6_mixed_success_and_skip_is_skipped() {
    assert_eq!(
        PlanExecutionOutcome::new(false, [S::Succeeded, S::Skipped]).status(),
        E::Skipped
    );
    report_only_control();
}

#[test]
fn c7_all_succeeded_stays_succeeded() {
    assert_eq!(
        PlanExecutionOutcome::new(false, [S::Succeeded, S::Succeeded]).status(),
        E::Succeeded
    );
    report_only_control();
}

#[test]
fn c8_failure_or_refusal_dominates() {
    for terminal in [S::Failed, S::Refused] {
        assert_eq!(
            PlanExecutionOutcome::new(false, [S::Succeeded, S::Skipped, terminal]).status(),
            E::Failed
        );
    }
    report_only_control();
}

#[test]
fn c9_dry_run_precedence_stays_same() {
    assert_eq!(
        PlanExecutionOutcome::new(true, [S::Failed, S::Skipped]).status(),
        E::DryRun
    );
    report_only_control();
}

#[test]
fn c10_dry_step_in_live_plan_cannot_succeed() {
    assert_eq!(
        PlanExecutionOutcome::new(false, [S::DryRun]).status(),
        E::Skipped
    );
    report_only_control();
}

#[test]
fn c11_two_execution_objects_stay_independent() {
    let a = PlanExecutionOutcome::new(false, [S::Skipped]);
    let b = PlanExecutionOutcome::new(false, [S::Succeeded]);
    assert_eq!(a.status(), E::Skipped);
    assert_eq!(b.status(), E::Succeeded);
    assert_eq!(a.status(), E::Skipped);
    report_only_control();
}

#[test]
fn c12_reconstructed_status_round_trips() -> TestResult {
    let status = PlanExecutionOutcome::new(false, [S::Skipped]).status();
    let bytes = serde_json::to_vec(&status)?;
    assert_eq!(bytes, b"\"skipped\"");
    assert_eq!(serde_json::from_slice::<E>(&bytes)?, E::Skipped);
    assert_eq!(serde_json::from_str::<E>("\"succeeded\"")?, E::Succeeded);
    report_only_control();
    Ok(())
}

#[test]
fn c13_unknown_status_refused() {
    assert!(serde_json::from_str::<E>("\"verified_by_magic\"").is_err());
    report_only_control();
}

#[test]
fn c14_report_only_chain_cannot_become_success() {
    let (step, code, _) = AssistReadbackOutcome::new(true, true).into_parts();
    let outcome = PlanExecutionOutcome::new(false, [step]).status();
    assert_eq!(
        (step, code, outcome),
        (
            S::Skipped,
            Some("ASSIST_CORRECTION_REPORT_ONLY"),
            E::Skipped
        )
    );
    assert_ne!(outcome, E::Succeeded);
    report_only_control();
}
