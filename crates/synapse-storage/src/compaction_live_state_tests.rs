//! Per-CF TTL contract (#43): live-state CFs are never compaction-expired,
//! the other formerly "never expiring" CFs are.

use rocksdb::CompactionDecision;

use super::*;

#[test]
fn must_fail_live_state_cf_has_no_compaction_ttl() {
    assert_eq!(ttl_ns_for_cf("CF_SESSIONS"), None);
}

#[test]
fn must_pass_stamped_cfs_have_a_compaction_ttl() {
    // Literal ns per the retention spec (6 h / 1 h / 6 h / 30 d), not derived
    // from the same DEFAULTS table the code reads.
    const HOUR_NS: u64 = 3_600_000_000_000;
    for (cf, expected_ns) in [
        ("CF_PROCESS_HISTORY", 6 * HOUR_NS),
        ("CF_OCR_CACHE", HOUR_NS),
        ("CF_TELEMETRY", 6 * HOUR_NS),
        ("CF_AGENT_TRANSCRIPTS", 30 * 24 * HOUR_NS),
    ] {
        assert_eq!(
            ttl_ns_for_cf(cf),
            Some(expected_ns),
            "{cf} must expire by ts_ns at its retention TTL"
        );
    }
}

#[test]
fn must_fail_legacy_row_without_ts_ns_is_kept_not_expired() {
    // No key codec for these CFs and no value timestamp: Missing -> Keep.
    for cf in ["CF_PROCESS_HISTORY", "CF_TELEMETRY", "CF_AGENT_TRANSCRIPTS"] {
        let ts = resolve_timestamp(cf, b"process_history/v1/act_launch/x/1", br#"{"pid":1}"#);
        assert_eq!(ts, RetentionTimestamp::Missing);
        assert!(matches!(ts.decision(100, 1_000), CompactionDecision::Keep));
    }
}
