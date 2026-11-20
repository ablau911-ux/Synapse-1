//! #318c frozen behaviour table (work method v3). One case per L row: the
//! background foreground guard sees a target that takes the foreground shortly
//! AFTER a posted action returned (delayed steal), and a posted scroll at a
//! claimed window goes through that guard at all. Not edited after the red run.

use super::{
    BACKGROUND_FOREGROUND_SETTLE_DEFAULT_MS, BACKGROUND_FOREGROUND_SETTLE_MAX_MS,
    background_foreground_settle_config_ms, background_foreground_settle_ms,
    posted_scroll_guard_window,
};

#[test]
fn background_foreground_settle_ms_table_318c() {
    let rows: [(&str, Option<&str>, u64, u64); 7] = [
        (
            "318c_L1_posted_action_waits_configured",
            Some("postmessage"),
            150,
            150,
        ),
        ("318c_L2_uia_pattern_reads_at_once", Some("uia"), 150, 0),
        ("318c_L3_failed_action_reads_at_once", None, 150, 0),
        ("318c_L4_empty_tier_reads_at_once", Some(""), 150, 0),
        (
            "318c_L5_lookalike_tier_reads_at_once",
            Some("PostMessage "),
            150,
            0,
        ),
        (
            "318c_L6_disabled_settle_reads_at_once",
            Some("postmessage"),
            0,
            0,
        ),
        (
            "318c_L7_configured_above_cap_is_capped",
            Some("postmessage"),
            60_000,
            BACKGROUND_FOREGROUND_SETTLE_MAX_MS,
        ),
    ];
    let mut failures = Vec::new();
    for (row, tier, configured, expected) in rows {
        let got = background_foreground_settle_ms(tier, configured);
        if got != expected {
            failures.push(format!("row {row}: {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#318c table rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn background_foreground_settle_config_ms_table_318c() {
    let rows: [(&str, Option<&str>, u64); 7] = [
        (
            "318c_C1_unset_uses_default",
            None,
            BACKGROUND_FOREGROUND_SETTLE_DEFAULT_MS,
        ),
        ("318c_C2_override_is_used", Some("80"), 80),
        ("318c_C3_zero_disables", Some("0"), 0),
        (
            "318c_C4_empty_uses_default",
            Some(""),
            BACKGROUND_FOREGROUND_SETTLE_DEFAULT_MS,
        ),
        (
            "318c_C5_garbage_uses_default",
            Some("-5ms"),
            BACKGROUND_FOREGROUND_SETTLE_DEFAULT_MS,
        ),
        (
            "318c_C6_above_cap_is_capped",
            Some("999999"),
            BACKGROUND_FOREGROUND_SETTLE_MAX_MS,
        ),
        (
            "318c_C7_cap_boundary_is_kept",
            Some("1000"),
            BACKGROUND_FOREGROUND_SETTLE_MAX_MS,
        ),
    ];
    let mut failures = Vec::new();
    for (row, raw, expected) in rows {
        let got = background_foreground_settle_config_ms(raw);
        if got != expected {
            failures.push(format!("row {row}: {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#318c table rows failed:\n{}",
        failures.join("\n")
    );
}

/// (row, element target, at point, checked window, expected guarded window).
type GuardWindowRow = (&'static str, bool, bool, Option<i64>, Option<i64>);

#[test]
fn posted_scroll_guard_window_table_318c() {
    let rows: [GuardWindowRow; 5] = [
        (
            "318c_G1_point_in_claimed_window_is_guarded",
            false,
            true,
            Some(0x1234),
            Some(0x1234),
        ),
        (
            "318c_G2_element_target_uses_its_own_guard",
            true,
            false,
            Some(0x1234),
            None,
        ),
        (
            "318c_G3_cursor_route_is_not_guarded",
            false,
            false,
            Some(0x1234),
            None,
        ),
        (
            "318c_G4_point_without_checked_window_is_not_guarded",
            false,
            true,
            None,
            None,
        ),
        (
            "318c_G5_invalid_point_and_element_together_is_not_guarded",
            true,
            true,
            Some(0x1234),
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (row, element_target, at_point, checked, expected) in rows {
        let got = posted_scroll_guard_window(element_target, at_point, checked);
        if got != expected {
            failures.push(format!("row {row}: {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#318c table rows failed:\n{}",
        failures.join("\n")
    );
}
