use super::{Priority, resolve};

#[test]
fn must_fail_unset_or_blank_boosts_only_workers() {
    assert_eq!(resolve(None), (Priority::WorkerAboveNormal, None));
    assert_eq!(resolve(Some("  ")), (Priority::WorkerAboveNormal, None));
}

#[test]
fn must_pass_known_values_are_honoured_case_insensitively() {
    assert_eq!(resolve(Some("normal")), (Priority::Normal, None));
    assert_eq!(resolve(Some("Above_Normal")), (Priority::AboveNormal, None));
    assert_eq!(resolve(Some("abovenormal")), (Priority::AboveNormal, None));
    assert_eq!(resolve(Some("HIGH")), (Priority::High, None));
}

#[test]
fn must_fail_unknown_value_falls_back_and_is_reported() {
    assert_eq!(
        resolve(Some("realtime")),
        (Priority::WorkerAboveNormal, Some("realtime".to_owned()))
    );
}

#[test]
fn must_fail_only_the_default_policy_boosts_workers() {
    assert!(Priority::WorkerAboveNormal.boosts_workers());
    assert_eq!(
        resolve(Some("worker_above_normal")),
        (Priority::WorkerAboveNormal, None)
    );
    for other in [Priority::Normal, Priority::AboveNormal, Priority::High] {
        assert!(!other.boosts_workers());
    }
}
