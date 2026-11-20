//! A latched storage batcher failure must be visible in health (#33).

use synapse_storage::BatcherStatus;

use super::{DEGRADED_STATUS, degraded_detail};

#[test]
fn must_pass_healthy_batcher_reports_no_degradation() {
    assert_eq!(degraded_detail(&BatcherStatus::default()), None);
}

#[test]
fn must_fail_failed_batcher_reports_reason_and_recovery_state() {
    let status = BatcherStatus {
        failed: true,
        detail: Some("WAL write denied".to_owned()),
        consecutive_failures: 3,
        recoveries: 1,
        retry_in_ms: Some(4_000),
    };
    let detail = degraded_detail(&status).expect("a latched failure is degraded");
    assert_eq!(
        detail,
        "storage write batcher failed and is refusing writes: WAL write denied \
         (consecutive_failures=3, recoveries=1, next_recovery_probe_in_ms=4000)"
    );
    // The health wire value for the storage subsystem while writes are refused.
    assert_eq!(DEGRADED_STATUS, "degraded_writes");
}
