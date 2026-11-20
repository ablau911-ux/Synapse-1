//! FU1 #14: configured tombstone capacity cannot exceed 4096.
use crate::server::terminated_sessions::TerminatedSessions;

#[test]
fn must_pass_capacity_accepts_one_and_4096_14() {
    for (raw, expected) in [("1", 1), ("4096", 4096), (" 4096 ", 4096)] {
        assert_eq!(TerminatedSessions::parse_capacity(raw).unwrap(), expected);
        let registry = TerminatedSessions::with_capacity(expected);
        assert_eq!(registry.stats().capacity, expected);
        assert_eq!(registry.stats().len, 0);
        assert_eq!(registry.stats().evicted_total, 0);
    }
}

#[test]
fn must_fail_capacity_above_4096_is_refused_without_registry_change_14() {
    let mut registry = TerminatedSessions::with_capacity(4096);
    assert!(registry.insert("retained".to_owned()));
    let before = registry.stats();
    for raw in ["4097", "1000000"] {
        assert_eq!(
            TerminatedSessions::parse_capacity(raw)
                .unwrap_err()
                .to_string(),
            format!("terminated session capacity must not exceed 4096, got {raw}")
        );
        assert_eq!(registry.stats(), before);
        assert!(registry.contains("retained"));
    }
    assert_eq!(
        TerminatedSessions::parse_capacity("0")
            .unwrap_err()
            .to_string(),
        "terminated session capacity must be greater than zero"
    );
    assert_eq!(registry.stats(), before);
}
