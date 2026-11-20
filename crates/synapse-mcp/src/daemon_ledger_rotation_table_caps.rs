//! #2330 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row: the ledger rotation caps come from
//! configuration, each with its default.

use super::LedgerRotation;

const DEFAULT_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_SEGMENTS: usize = 5;

const fn caps(segment_bytes: u64, segments: usize) -> LedgerRotation {
    LedgerRotation {
        segment_bytes,
        segments,
    }
}

#[test]
fn ledger_rotation_caps_table_2330() {
    // (row, segment bytes setting, segments setting, expected caps)
    let rows: [(&str, Option<&str>, Option<&str>, LedgerRotation); 16] = [
        (
            "C1_empty_both_unset_gives_defaults",
            None,
            None,
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C2_normal_both_set",
            Some("1048576"),
            Some("3"),
            caps(1_048_576, 3),
        ),
        (
            "C3_boundary_lowest_accepted",
            Some("65536"),
            Some("1"),
            caps(65_536, 1),
        ),
        (
            "C3_boundary_highest_accepted",
            Some("1073741824"),
            Some("50"),
            caps(1_073_741_824, 50),
        ),
        (
            "C4_boundary_below_range_keeps_defaults",
            Some("65535"),
            Some("0"),
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C4_boundary_above_range_keeps_defaults",
            Some("1073741825"),
            Some("51"),
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C5_hostile_empty_strings",
            Some(""),
            Some(""),
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C5_hostile_words",
            Some("abc"),
            Some("many"),
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C5_hostile_negative",
            Some("-1"),
            Some("-3"),
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C5_hostile_unit_suffix",
            Some("8MiB"),
            Some("5 files"),
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C5_hostile_number_too_long",
            Some("99999999999999999999999"),
            Some("99999999999999999999999"),
            caps(DEFAULT_BYTES, DEFAULT_SEGMENTS),
        ),
        (
            "C6_spaces_around_the_number_are_ignored",
            Some(" 2097152 "),
            Some(" 4 "),
            caps(2_097_152, 4),
        ),
        (
            "C7_each_value_on_its_own_bad_segments",
            Some("1048576"),
            Some("x"),
            caps(1_048_576, DEFAULT_SEGMENTS),
        ),
        (
            "C7_each_value_on_its_own_bad_bytes",
            Some("x"),
            Some("3"),
            caps(DEFAULT_BYTES, 3),
        ),
        (
            "C7_each_value_on_its_own_only_bytes_set",
            Some("1048576"),
            None,
            caps(1_048_576, DEFAULT_SEGMENTS),
        ),
        (
            "C7_each_value_on_its_own_only_segments_set",
            None,
            Some("9"),
            caps(DEFAULT_BYTES, 9),
        ),
    ];
    let mut failures = Vec::new();
    for (row, segment_bytes, segments, expected) in rows {
        let got = LedgerRotation::from_settings(segment_bytes, segments);
        if got != expected {
            failures.push(format!("row {row}: caps {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2330 caps rows failed:\n{}",
        failures.join("\n")
    );
}
