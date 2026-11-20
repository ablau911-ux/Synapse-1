//! #144: an unqueryable (ACCESS_DENIED) reused PID must never hide an unclean
//! previous run. Liveness decisions are driven through injected probes.
use super::*;

fn run(pid: u32, started_at_unix_ms: u64, process_started_at_100ns: Option<u64>) -> RunRecord {
    RunRecord {
        schema_version: SCHEMA_VERSION,
        run_id: "previous".into(),
        pid,
        process_started_at_100ns,
        mode: "http".into(),
        bind_addr: None,
        db_path: "test".into(),
        started_at_unix_ms,
        ended_at_unix_ms: None,
        ended_reason: None,
    }
}

fn denied(_pid: u32) -> ProcessProbe {
    ProcessProbe::Unknown("access denied".into())
}

fn liveness(
    record: &RunRecord,
    boot: Option<u64>,
    probe: impl FnOnce(u32) -> ProcessProbe,
) -> Option<PreviousLiveness> {
    unclean_previous_run_with(Some(record), None, false, boot, probe).map(|(_, _, state)| state)
}

#[test]
fn must_pass_run_started_before_boot_is_dead_even_when_pid_cannot_be_opened() {
    let record = run(4, 1_000, None);
    let probed = std::cell::Cell::new(false);
    let state = liveness(&record, Some(5_000), |_| {
        probed.set(true);
        ProcessProbe::Unknown("access denied".into())
    });
    assert_eq!(state, Some(PreviousLiveness::Dead));
    assert!(!probed.get(), "boot time alone decides; no probe needed");
}

#[test]
fn must_fail_unqueryable_pid_after_boot_records_unknown_not_nothing() {
    let record = run(4, 9_000, Some(7));
    assert_eq!(
        liveness(&record, Some(5_000), denied),
        Some(PreviousLiveness::Unknown)
    );
    assert_eq!(
        liveness(&record, None, denied),
        Some(PreviousLiveness::Unknown)
    );
}

#[test]
fn must_fail_boot_boundary_run_started_exactly_at_boot_still_probes() {
    let record = run(4, 5_000, None);
    assert_eq!(
        liveness(&record, Some(5_000), denied),
        Some(PreviousLiveness::Unknown)
    );
}

#[test]
fn must_pass_matching_creation_time_or_legacy_record_is_alive_and_not_reported() {
    let running = |_| ProcessProbe::Running { created_100ns: 7 };
    assert_eq!(
        liveness(&run(4, 9_000, Some(7)), Some(5_000), running),
        None
    );
    assert_eq!(liveness(&run(4, 9_000, None), Some(5_000), running), None);
}

#[test]
fn must_pass_creation_time_mismatch_or_gone_process_is_dead() {
    let other = |_| ProcessProbe::Running { created_100ns: 8 };
    assert_eq!(
        liveness(&run(4, 9_000, Some(7)), Some(5_000), other),
        Some(PreviousLiveness::Dead)
    );
    assert_eq!(
        liveness(&run(4, 9_000, Some(7)), Some(5_000), |_| ProcessProbe::Gone),
        Some(PreviousLiveness::Dead)
    );
}

#[test]
fn must_fail_recorded_exit_or_ended_run_is_never_reported_even_if_unknown() {
    let record = run(4, 9_000, None);
    assert!(unclean_previous_run_with(Some(&record), None, true, None, denied).is_none());
    let mut ended = record;
    ended.ended_at_unix_ms = Some(9_500);
    assert!(unclean_previous_run_with(Some(&ended), None, false, None, denied).is_none());
}

#[test]
fn must_pass_unknown_gets_its_own_event_kind_and_cause() {
    let (kind, cause, _) = unclean_event_labels(PreviousLiveness::Unknown);
    assert_eq!(
        (kind, cause),
        ("previous_run_unknown", "previous_process_unverifiable")
    );
    let (kind, cause, _) = unclean_event_labels(PreviousLiveness::Dead);
    assert_eq!(
        (kind, cause),
        ("previous_run_unclean", "process_missing_on_startup")
    );
}
