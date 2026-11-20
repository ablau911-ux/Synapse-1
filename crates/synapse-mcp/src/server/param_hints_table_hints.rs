//! Parameter-hints frozen behaviour table (manager-owned, work method v3).
//! Codex and Opus never edit this file. One case per L row of the item 4
//! table in the tab-locks lane report.

use super::{DESERIALIZE_PREFIX, merge_target_alias, param_hint_message};

/// Expected hint: `None` = the raw message stands; `Some(must, must_not)`.
type Expect = Option<(&'static [&'static str], &'static [&'static str])>;
/// (row, envelope target, spec target, single-target operation, expected).
type MergeRow = (
    &'static str,
    Option<String>,
    Option<String>,
    bool,
    Result<Option<String>, &'static str>,
);

fn unknown(field: &str, accepted: &[&str]) -> String {
    let list = accepted
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{DESERIALIZE_PREFIX} unknown field `{field}`, expected one of {list}")
}

#[test]
fn param_hints_table_hints() {
    let oversized = "x".repeat(5_000);
    let hint_rows: [(&str, &str, String, Expect); 12] = [
        (
            "4a_camel_case_field_suggests_snake_case",
            "browser_debugger",
            unknown(
                "awaitPromise",
                &["expression", "await_promise", "cdp_target_id"],
            ),
            Some((&["did you mean `await_promise`", "browser_debugger"], &[])),
        ),
        (
            "4a_near_spelling_suggests_accepted_field",
            "browser_wait",
            unknown("opration", &["operation", "wait"]),
            Some((&["did you mean `operation`"], &[])),
        ),
        (
            "4a_case_and_underscore_fold_suggests_field",
            "browser_capture",
            unknown("CDPTargetId", &["operation", "screenshot", "cdp_target_id"]),
            Some((&["did you mean `cdp_target_id`"], &[])),
        ),
        (
            "4b_empty_field_name_gets_no_hint",
            "browser_wait",
            unknown("", &["operation", "wait"]),
            None,
        ),
        (
            "4b_unrelated_field_gets_no_fabricated_hint",
            "browser_wait",
            unknown("zzzzzzzzzz", &["operation", "wait"]),
            None,
        ),
        (
            "4b_ambiguous_field_gets_no_hint",
            "browser_form",
            unknown("ab", &["aa", "bb"]),
            None,
        ),
        (
            "4c_hoisted_wait_budget_points_to_nested_spec",
            "browser_wait",
            unknown("timeout_ms", &["operation", "wait"]),
            Some((&["wait.text.timeout_ms"], &[])),
        ),
        (
            "4c_operation_value_used_as_field_points_to_operation",
            "profile",
            unknown(
                "set",
                &["operation", "profile", "reason", "confirm_break_glass"],
            ),
            Some((&["operation=set"], &[])),
        ),
        (
            "4c_act_target_field_points_to_public_target_operation",
            "act",
            unknown(
                "cdp_target_id",
                &["operation", "action", "reason", "ttl_ms"],
            ),
            Some((&["target operation=set"], &["set_target"])),
        ),
        (
            "4d_hostile_oversized_field_is_bounded",
            "browser_wait",
            unknown(&oversized, &["operation", "wait"]),
            Some((&["browser_wait", "unknown field"], &[])),
        ),
        (
            "4e_domain_error_is_not_touched",
            "browser_wait",
            "cdp_target_id must be a 32-character hex id".to_owned(),
            None,
        ),
        (
            "4e_type_mismatch_is_not_touched",
            "browser_wait",
            format!("{DESERIALIZE_PREFIX} invalid type: string \"secret-value\", expected u64"),
            None,
        ),
    ];

    let mut failures = Vec::new();
    for (row, tool, raw, expected) in &hint_rows {
        let got = param_hint_message(tool, raw);
        match (expected, &got) {
            (None, None) => {}
            (Some((must, must_not)), Some(message)) => {
                for fragment in *must {
                    if !message.contains(fragment) {
                        failures.push(format!("row {row}: hint lacks {fragment:?}: {message}"));
                    }
                }
                for fragment in *must_not {
                    if message.contains(fragment) {
                        failures.push(format!("row {row}: hint has {fragment:?}: {message}"));
                    }
                }
                if message.len() > 512 {
                    failures.push(format!(
                        "row {row}: hint is {} bytes, bound is 512",
                        message.len()
                    ));
                }
            }
            _ => failures.push(format!("row {row}: hint {got:?}, expected {expected:?}")),
        }
    }

    // Merge rows: (row, envelope, spec, single-target operation, expected).
    // Err(fragment) = refused with a message carrying fragment and neither value.
    let tab_a = || Some("TABAAAA".to_owned());
    let tab_b = || Some("TABBBBB".to_owned());
    let merge_rows: [MergeRow; 7] = [
        (
            "4g_envelope_target_only_is_used",
            tab_a(),
            None,
            true,
            Ok(tab_a()),
        ),
        (
            "4g_identical_envelope_and_spec_target_is_one_target",
            tab_a(),
            tab_a(),
            true,
            Ok(tab_a()),
        ),
        (
            "4g_neighbour_spec_target_only_is_unchanged",
            None,
            tab_b(),
            true,
            Ok(tab_b()),
        ),
        (
            "4g_neighbour_no_target_stays_none",
            None,
            None,
            true,
            Ok(None),
        ),
        (
            "4h_conflicting_targets_are_refused",
            tab_a(),
            tab_b(),
            true,
            Err("conflicts"),
        ),
        (
            "4h_case_different_targets_are_refused",
            tab_a(),
            Some("tabaaaa".to_owned()),
            true,
            Err("conflicts"),
        ),
        (
            "4i_alias_on_reload_bridge_is_refused",
            tab_a(),
            None,
            false,
            Err("single target"),
        ),
    ];
    for (row, envelope, spec, single, expected) in merge_rows {
        let values: Vec<String> = envelope.iter().chain(spec.iter()).cloned().collect();
        let got = merge_target_alias("browser_debugger", "cdp_target_id", envelope, spec, single);
        match (&expected, &got) {
            (Ok(want), Ok(have)) if want == have => {}
            (Err(fragment), Err(message)) => {
                if !message.contains(fragment) {
                    failures.push(format!("row {row}: refusal lacks {fragment:?}: {message}"));
                }
                for value in &values {
                    if message.contains(value.as_str()) {
                        failures.push(format!("row {row}: refusal echoes {value:?}: {message}"));
                    }
                }
            }
            _ => failures.push(format!("row {row}: merge {got:?}, expected {expected:?}")),
        }
    }
    let hwnd = merge_target_alias(
        "browser_wait",
        "window_hwnd",
        Some(0x10_i64),
        Some(0x20),
        true,
    );
    if !hwnd
        .as_ref()
        .is_err_and(|message| message.contains("conflicts"))
    {
        failures.push(format!(
            "row 4h_conflicting_window_hwnd_is_refused: merge {hwnd:?}, expected conflict"
        ));
    }

    assert!(
        failures.is_empty(),
        "parameter-hints table rows failed:\n{}",
        failures.join("\n")
    );
}
