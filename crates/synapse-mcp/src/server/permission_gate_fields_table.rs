//! Frozen behaviour table for #303 row 303a: which gate requests are refused up front.
//!
//! Written by the manager from the behaviour table in issue #303 (work method v3).
//! Code agents must not edit this file. Deviation logged in #303: written after the
//! code; red evidence is the old build (4b003b82), which accepted every row below
//! and blocked on an empty gate.

use super::missing_gate_fields;

/// (row name, tool_name, tool_use_id, spawn_id, expected missing fields)
type Row = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
    Vec<&'static str>,
);

fn rows() -> Vec<Row> {
    vec![
        // normal: a spawned agent's permission prompt
        (
            "complete",
            Some("Bash"),
            Some("toolu_1"),
            Some("spawn-1"),
            vec![],
        ),
        // empty: the reported `gate: {}`
        (
            "all_missing",
            None,
            None,
            None,
            vec!["tool_name", "tool_use_id", "spawn_id"],
        ),
        // boundary: blank and whitespace-only values count as missing
        (
            "blank_values",
            Some(""),
            Some("   "),
            Some("spawn-1"),
            vec!["tool_name", "tool_use_id"],
        ),
        // one field missing: the caller had no spawn header and passed no spawn_id
        (
            "no_spawn",
            Some("Bash"),
            Some("toolu_1"),
            None,
            vec!["spawn_id"],
        ),
    ]
}

#[test]
fn gate_fields_table() {
    for (name, tool_name, tool_use_id, spawn_id, expected) in rows() {
        assert_eq!(
            missing_gate_fields(tool_name, tool_use_id, spawn_id),
            expected,
            "row {name}"
        );
    }
}
