//! Key-embedded timestamp fallback and TTL-scaled periodic compaction (#43).

use rocksdb::CompactionDecision;

use super::*;

const TTL_NS: u64 = 100;
const NOW_NS: u64 = 1_000;

fn ts_key(ts_ns: u64) -> Vec<u8> {
    let mut key = ts_ns.to_be_bytes().to_vec();
    key.extend_from_slice(&7_u32.to_be_bytes());
    key
}

fn decide(cf: &str, key: &[u8], value: &[u8]) -> CompactionDecision {
    resolve_timestamp(cf, key, value).decision(TTL_NS, NOW_NS)
}

#[test]
fn must_pass_rows_without_value_timestamp_expire_by_key_timestamp() {
    for value in [
        br#"{"payload":"no root ts"}"#.as_slice(),
        b"\x00\x01 not json".as_slice(),
        br#"{"ts_ns":"0"}"#.as_slice(),
    ] {
        assert!(matches!(
            decide("CF_TIMELINE", &ts_key(10), value),
            CompactionDecision::Remove
        ));
        assert!(matches!(
            decide("CF_TIMELINE", &ts_key(950), value),
            CompactionDecision::Keep
        ));
    }
}

#[test]
fn must_pass_value_timestamp_wins_over_key_timestamp() {
    // Fresh value ts, ancient key ts -> keep; ancient value ts, fresh key -> remove.
    assert!(matches!(
        decide("CF_EVENTS", &ts_key(0), br#"{"ts_ns":990}"#),
        CompactionDecision::Keep
    ));
    assert!(matches!(
        decide("CF_EVENTS", &ts_key(990), br#"{"ts_ns":0}"#),
        CompactionDecision::Remove
    ));
}

#[test]
fn must_fail_keys_that_do_not_match_the_codec_keep_the_row() {
    let value = br#"{"payload":1}"#;
    // Wrong length for the big-endian codec.
    assert!(matches!(
        decide("CF_TIMELINE", &[0_u8; 11], value),
        CompactionDecision::Keep
    ));
    assert!(matches!(
        decide("CF_TIMELINE", b"", value),
        CompactionDecision::Keep
    ));
    // CF without a key timestamp format.
    assert!(matches!(
        decide("CF_SESSIONS", &ts_key(1), value),
        CompactionDecision::Keep
    ));
    assert!(KeyTimestampCodec::for_cf("CF_OCR_CACHE").is_none());
}

#[test]
fn must_fail_reflex_audit_text_key_timestamp_is_parsed_strictly() {
    let value = b"not json";
    let old = format!("rfx_abc:{:020}:aud_1", 5_u64);
    let fresh = format!("rfx_abc:{:020}:aud_1", 990_u64);
    assert!(matches!(
        decide("CF_REFLEX_AUDIT", old.as_bytes(), value),
        CompactionDecision::Remove
    ));
    assert!(matches!(
        decide("CF_REFLEX_AUDIT", fresh.as_bytes(), value),
        CompactionDecision::Keep
    ));
    for malformed in [
        "rfx_abc:5:aud_1".to_owned(),
        "rfx_abc".to_owned(),
        format!("rfx_abc:{}x:aud_1", "0".repeat(19)),
        String::new(),
    ] {
        assert!(matches!(
            decide("CF_REFLEX_AUDIT", malformed.as_bytes(), value),
            CompactionDecision::Keep
        ));
    }
    assert!(matches!(
        decide("CF_REFLEX_AUDIT", &[0xff, 0xfe, b':'], value),
        CompactionDecision::Keep
    ));
}

#[test]
fn must_pass_periodic_compaction_scales_with_ttl_within_bounds() {
    let hours = |h: u64| h * 3_600 * NANOS_PER_SECOND;
    let days = |d: u64| d * 24 * 3_600 * NANOS_PER_SECOND;
    // Short TTL: floor of one hour.
    assert_eq!(periodic_compaction_seconds(hours(1)), 3_600);
    // 24 h TTL -> 6 h.
    assert_eq!(periodic_compaction_seconds(hours(24)), 6 * 3_600);
    // 90 d TTL is no longer rewritten daily.
    let ninety = periodic_compaction_seconds(days(90));
    assert!(
        ninety > 86_400,
        "90-day CF must not compact daily: {ninety}"
    );
    assert_eq!(ninety, 90 * 24 * 3_600 / 4);
    // Upper clamp.
    assert_eq!(periodic_compaction_seconds(days(3_650)), 30 * 24 * 3_600);
}
