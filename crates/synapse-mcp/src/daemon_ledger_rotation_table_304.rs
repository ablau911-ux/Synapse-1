//! #304 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the table in Gitea issue #304.

use super::{
    LedgerAppendPlan, LedgerRotation, MAX_LEDGER_SEGMENT_BYTES, MAX_LEDGER_SEGMENTS, RotationStep,
};

const LIMIT: u64 = 1_000;
const ACTIVE: &str = "daemon-tool-events.jsonl";

#[test]
fn ledger_append_plan_table_304() {
    use LedgerAppendPlan::{Append, RotateThenAppend};
    // (row, active ledger bytes, next record bytes, segment limit, expected plan)
    let rows: [(&str, u64, u64, u64, LedgerAppendPlan); 9] = [
        ("304a_normal_below_limit_appends", 100, 50, LIMIT, Append),
        (
            "304b_empty_ledger_first_record_appends",
            0,
            50,
            LIMIT,
            Append,
        ),
        (
            "304c_boundary_exactly_at_limit_appends",
            950,
            50,
            LIMIT,
            Append,
        ),
        (
            "304c_boundary_one_byte_over_limit_rotates",
            951,
            50,
            LIMIT,
            RotateThenAppend,
        ),
        (
            "304d_hostile_oversized_record_into_empty_ledger_is_kept",
            0,
            5_000,
            LIMIT,
            Append,
        ),
        (
            "304d_hostile_oversized_record_into_used_ledger_rotates_first",
            10,
            5_000,
            LIMIT,
            RotateThenAppend,
        ),
        (
            "304d_hostile_counter_overflow_rotates_without_panic",
            u64::MAX,
            10,
            LIMIT,
            RotateThenAppend,
        ),
        (
            "304e_restart_active_ledger_already_at_limit_rotates_on_first_record",
            LIMIT,
            1,
            LIMIT,
            RotateThenAppend,
        ),
        (
            "304e_restart_active_ledger_over_limit_rotates_on_first_record",
            LIMIT * 3,
            1,
            LIMIT,
            RotateThenAppend,
        ),
    ];
    let mut failures = Vec::new();
    for (row, active, record, limit, expected) in rows {
        let rotation = LedgerRotation {
            segment_bytes: limit,
            segments: MAX_LEDGER_SEGMENTS,
        };
        let got = rotation.plan(active, record);
        if got != expected {
            failures.push(format!("row {row}: plan {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#304 append-plan rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn ledger_rotation_steps_table_304() {
    use RotationStep::{DeleteSegment, RenameSegment, RetireActive};
    // (row, retained segment count, expected ordered steps)
    let rows: [(&str, usize, Vec<RotationStep>); 2] = [
        (
            "304f_retention_drops_oldest_then_shifts_then_retires_active",
            5,
            vec![
                DeleteSegment(5),
                RenameSegment { from: 4, to: 5 },
                RenameSegment { from: 3, to: 4 },
                RenameSegment { from: 2, to: 3 },
                RenameSegment { from: 1, to: 2 },
                RetireActive,
            ],
        ),
        (
            "304f_retention_single_segment_replaces_it",
            1,
            vec![DeleteSegment(1), RetireActive],
        ),
    ];
    let mut failures = Vec::new();
    for (row, max_segments, expected) in rows {
        let rotation = LedgerRotation {
            segment_bytes: MAX_LEDGER_SEGMENT_BYTES,
            segments: max_segments,
        };
        let got = rotation.steps();
        if got != expected {
            failures.push(format!("row {row}: steps {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#304 rotation-step rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn ledger_segment_names_table_304() {
    let mut failures = Vec::new();
    // (row, segment index, expected file name)
    let names: [(&str, usize, &str); 2] = [
        ("304g_segment_one_name", 1, "daemon-tool-events.jsonl.1"),
        ("304g_segment_five_name", 5, "daemon-tool-events.jsonl.5"),
    ];
    for (row, index, expected) in names {
        let got = LedgerRotation::segment_file_name(ACTIVE, index);
        if got != expected {
            failures.push(format!("row {row}: name {got:?}, expected {expected:?}"));
        }
    }
    // 304h someone else's file: the rotation for the shipped retention only
    // ever names `.1`..`.5` of the tool-event ledger, never the active file
    // itself, the exit ledger, a backup or a neighbour segment.
    let produced: Vec<String> = (1..=5)
        .map(|index| LedgerRotation::segment_file_name(ACTIVE, index))
        .collect();
    for foreign in [
        ACTIVE,
        "daemon-exit.jsonl",
        "daemon-tool-events.jsonl.0",
        "daemon-tool-events.jsonl.6",
        "daemon-tool-events.jsonl.bak",
    ] {
        if produced.iter().any(|name| name == foreign) {
            failures.push(format!(
                "row 304h_foreign_names_are_never_produced: produced {foreign:?}"
            ));
        }
    }
    let unique: std::collections::BTreeSet<&String> = produced.iter().collect();
    if unique.len() != produced.len() {
        failures.push(format!(
            "row 304h_foreign_names_are_never_produced: names not distinct {produced:?}"
        ));
    }
    assert!(
        failures.is_empty(),
        "#304 segment-name rows failed:\n{}",
        failures.join("\n")
    );
}
