//! Frozen behaviour table for lane memory-4 rows L1-L27 (action history pure core).
//! Written by the lane manager from
//! `.claude/synapse-agents/merge/jobs/lane-memory-4/table.md` (work method v3).
//! Code agents must not edit this file.
//!
//! One case per row, each case named after its row. Failures are collected per
//! row so one run shows every failing row. Rows whose main expectation is an
//! absence (no match, no fragment, empty) carry a contrast check that a stub
//! cannot pass.

use serde_json::{Value, json};

use crate::action_history::{
    ACTION_HISTORY_PREFIX, ACTION_HISTORY_RECORD_VERSION, ActionHistoryConfig, ActionHistoryRow,
    FinishedCall, HistoryQuery, outcome_label, resolve_operation,
};
use crate::retention::{DEFAULTS, KV_NAMESPACE_QUOTAS};

type Check = Result<(), String>;

const T: u64 = 1_700_000_000_123;
const MS_PER_DAY: u64 = 86_400_000;
const APP: &str = "notepad.exe";
const TITLE: &str = "Untitled - Notepad";

macro_rules! rows {
    ($($case:ident),* $(,)?) => {
        &[$((stringify!($case), $case as fn() -> Check)),*]
    };
}

fn run(rows: &[(&str, fn() -> Check)]) {
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|(row, case)| case().err().map(|why| format!("{row}: {why}")))
        .collect();
    assert!(
        failures.is_empty(),
        "{} failing row(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn check(ok: bool, why: impl FnOnce() -> String) -> Check {
    if ok { Ok(()) } else { Err(why()) }
}

fn same(redacted: &str) -> String {
    redacted.to_owned()
}

fn call<'a>(app: &'a str, title: &'a str, args: Option<&'a Value>) -> FinishedCall<'a> {
    FinishedCall {
        ts_ms: T,
        session_id: "S1",
        tool: "act",
        operation: "invoke",
        target_app: app,
        target_title: title,
        outcome: "ok",
        args,
    }
}

fn click_args() -> Value {
    json!({"operation": "invoke", "action": {"verb": "click", "element_id": "e1"}})
}

fn fixture_row(ts_ms: u64) -> ActionHistoryRow {
    ActionHistoryRow {
        record_version: ACTION_HISTORY_RECORD_VERSION,
        ts_ms,
        session_id: "sess-A".to_owned(),
        tool: "act".to_owned(),
        operation: "invoke".to_owned(),
        target: format!("{APP} - {TITLE}"),
        outcome: "ok".to_owned(),
        summary: format!("act invoke verb=click on {APP} - {TITLE} -> ok"),
    }
}

fn words(query: &str) -> Vec<String> {
    HistoryQuery::new(query, None, None, None).words
}

// ---- row building -------------------------------------------------------

fn l1_normal_row_has_every_field() -> Check {
    let args = click_args();
    let row = ActionHistoryRow::build(&call(APP, TITLE, Some(&args)), &same);
    let expected = ActionHistoryRow {
        record_version: 1,
        ts_ms: T,
        session_id: "S1".to_owned(),
        tool: "act".to_owned(),
        operation: "invoke".to_owned(),
        target: "notepad.exe - Untitled - Notepad".to_owned(),
        outcome: "ok".to_owned(),
        summary: "act invoke verb=click element_id=e1 on notepad.exe - Untitled - Notepad -> ok"
            .to_owned(),
    };
    check(row == expected, || {
        format!("expected {expected:?}, got {row:?}")
    })
}

fn l2_no_target_gives_empty_target_and_no_on() -> Check {
    let args = click_args();
    let row = ActionHistoryRow::build(&call("", "", Some(&args)), &same);
    check(
        row.target.is_empty() && row.summary == "act invoke verb=click element_id=e1 -> ok",
        || format!("expected empty target and plain summary, got {row:?}"),
    )
}

fn l3_partial_target_keeps_the_present_part() -> Check {
    let cases = [
        (APP, "", APP.to_owned()),
        ("", TITLE, TITLE.to_owned()),
        ("  ", TITLE, TITLE.to_owned()),
        ("a.exe", "b", "a.exe - b".to_owned()),
        ("", "", String::new()),
    ];
    for (app, title, expected) in cases {
        let got = ActionHistoryRow::target_line(app, title);
        if got != expected {
            return Err(format!(
                "history_target({app:?}, {title:?}) = {got:?}, expected {expected:?}"
            ));
        }
    }
    Ok(())
}

fn l4_missing_or_non_object_args_give_bare_summary() -> Check {
    let empty = json!({});
    let text = json!("click");
    let list = json!([1, 2]);
    for args in [None, Some(&empty), Some(&text), Some(&list)] {
        let row = ActionHistoryRow::build(&call("", "", args), &same);
        if row.summary != "act invoke -> ok" {
            return Err(format!("args {args:?}: summary {:?}", row.summary));
        }
    }
    Ok(())
}

fn l5_outcome_label_is_ok_code_or_error() -> Check {
    let cases = [
        (false, Some("TOOL_PARAMS_INVALID"), "ok"),
        (false, None, "ok"),
        (true, Some("TOOL_PARAMS_INVALID"), "TOOL_PARAMS_INVALID"),
        (true, None, "error"),
        (true, Some("  "), "error"),
    ];
    for (failed, code, expected) in cases {
        let got = outcome_label(failed, code);
        if got != expected {
            return Err(format!(
                "outcome_label({failed}, {code:?}) = {got:?}, expected {expected:?}"
            ));
        }
    }
    Ok(())
}

fn l6_summary_boundary_200_kept_201_cut() -> Check {
    let exact = "a".repeat(200);
    let over = "a".repeat(201);
    let exact_out = ActionHistoryRow::one_line(&exact, 200);
    let over_out = ActionHistoryRow::one_line(&over, 200);
    let args = json!({"path": "p".repeat(300)});
    let row = ActionHistoryRow::build(&call("", "", Some(&args)), &same);
    check(
        exact_out == exact && over_out == exact && row.summary.chars().count() == 200,
        || {
            format!(
                "200 -> {} chars, 201 -> {} chars, long summary {} chars",
                exact_out.chars().count(),
                over_out.chars().count(),
                row.summary.chars().count()
            )
        },
    )
}

fn l7_multibyte_cut_on_a_character_boundary() -> Check {
    let text = "שלום🙂".repeat(60);
    let out = ActionHistoryRow::one_line(&text, 200);
    let expected: String = text.chars().take(200).collect();
    check(out == expected && out.chars().count() == 200, || {
        format!("got {} chars, expected the first 200", out.chars().count())
    })
}

fn l8_control_characters_become_spaces() -> Check {
    let args = json!({"action": {"verb": "type", "name": "a\nb\tc\u{0}d"}});
    let row = ActionHistoryRow::build(&call("", "", Some(&args)), &same);
    let line = ActionHistoryRow::one_line("  x\ny  ", 10);
    check(
        row.summary == "act invoke verb=type name=a b c d -> ok" && line == "x y",
        || format!("summary {:?}, line {line:?}", row.summary),
    )
}

fn l9_every_text_field_goes_through_redaction() -> Check {
    let redact = |text: &str| text.replace("MARK", "[R]");
    let args = json!({"verb": "MARKverb"});
    let marked = FinishedCall {
        ts_ms: T,
        session_id: "S-MARK",
        tool: "toolMARK",
        operation: "opMARK",
        target_app: "MARK.exe",
        target_title: "Title MARK",
        outcome: "MARK_CODE",
        args: Some(&args),
    };
    let row = ActionHistoryRow::build(&marked, &redact);
    let fields = [
        &row.session_id,
        &row.tool,
        &row.operation,
        &row.target,
        &row.outcome,
        &row.summary,
    ];
    check(
        fields.iter().all(|field| !field.contains("MARK"))
            && row.session_id == "S-[R]"
            && row.summary.contains("verb=[R]verb"),
        || format!("a field kept the marker or lost its text: {row:?}"),
    )
}

fn l10_redaction_runs_before_the_cut() -> Check {
    let redact = |text: &str| {
        text.split(' ')
            .map(|word| if word == "SECRETWORD" { "[R]" } else { word })
            .collect::<Vec<_>>()
            .join(" ")
    };
    // "act invoke path=" is 16 characters; the secret starts at character 195.
    let value = format!("{} SECRETWORD", "x".repeat(178));
    let args = json!({"path": value});
    let row = ActionHistoryRow::build(&call("", "", Some(&args)), &redact);
    check(
        !row.summary.contains("SECRE") && row.summary.contains("[R]"),
        || format!("summary {:?}", row.summary),
    )
}

fn l11_typed_content_never_enters_the_summary() -> Check {
    let args = json!({
        "action": {"verb": "type", "text": "my private words", "value": "hidden value"},
        "args": ["--password", "p4ss"]
    });
    let summary = ActionHistoryRow::summary_line("act", "invoke", "", Some(&args), "ok");
    check(summary == "act invoke verb=type -> ok", || {
        format!("summary {summary:?}")
    })
}

// Row L28 (manager, 2026-11-19; found by the Opus pass on 6b10f477): content
// inside a child object of the call (a stored `value` object, a shell `env`)
// never enters the summary.
fn l28_child_object_content_never_enters_the_summary() -> Check {
    let storage = json!({"operation": "set", "key": "k1", "value": {"path": "C:/private/notes.txt", "name": "secret"}});
    let shell = json!({"operation": "run", "command": "echo", "env": {"title": "token-title"}});
    let storage_summary =
        ActionHistoryRow::summary_line("storage", "set", "", Some(&storage), "ok");
    let shell_summary = ActionHistoryRow::summary_line("shell", "run", "", Some(&shell), "ok");
    check(
        storage_summary == "storage set key=k1 -> ok"
            && shell_summary == "shell run command=echo -> ok",
        || format!("storage {storage_summary:?}, shell {shell_summary:?}"),
    )
}

// ---- keys and retention -------------------------------------------------

fn l12_key_layout() -> Check {
    let key = fixture_row(T).key(7);
    check(
        key == "action-history/v1/00000001700000000123/0000000007",
        || format!("key {key:?}"),
    )
}

fn l13_keys_sort_by_time_then_sequence() -> Check {
    let ordered = fixture_row(T).key(9) < fixture_row(T + 1).key(0)
        && fixture_row(T).key(1) < fixture_row(T).key(2)
        && ActionHistoryRow::time_key(T) <= fixture_row(T).key(0)
        && ActionHistoryRow::time_key(T) > fixture_row(T - 1).key(u32::MAX)
        && ActionHistoryRow::time_key(T).starts_with(ACTION_HISTORY_PREFIX);
    check(ordered, || {
        format!(
            "keys out of order: time_key(T)={:?} key(T,0)={:?}",
            ActionHistoryRow::time_key(T),
            fixture_row(T).key(0)
        )
    })
}

fn l14_extreme_times_keep_the_layout() -> Check {
    let low = fixture_row(0).key(0);
    let high = fixture_row(u64::MAX).key(u32::MAX);
    check(
        low.len() == high.len()
            && low.starts_with(ACTION_HISTORY_PREFIX)
            && high == format!("{ACTION_HISTORY_PREFIX}18446744073709551615/4294967295"),
        || format!("low {low:?}, high {high:?}"),
    )
}

fn l15_retention_cutoff() -> Check {
    let cases = [
        (T, 90, T - 90 * MS_PER_DAY),
        (1_000, 90, 0),
        (T, u64::MAX, 0),
    ];
    for (now, days, expected) in cases {
        let config = ActionHistoryConfig {
            retention_days: days,
            ..ActionHistoryConfig::default()
        };
        let got = config.cutoff_ms(now);
        if got != expected {
            return Err(format!(
                "retention_cutoff_ms({now}, {days}) = {got}, expected {expected}"
            ));
        }
    }
    Ok(())
}

fn l16_config_default_and_overrides() -> Check {
    let shipped = ActionHistoryConfig {
        retention_days: 90,
        quota_prune_rows: 512,
    };
    let lookup = |days: &'static str, prune: &'static str| {
        ActionHistoryConfig::from_lookup(move |name| match name {
            "SYNAPSE_ACTION_HISTORY_RETENTION_DAYS" => Some(days.to_owned()),
            "SYNAPSE_ACTION_HISTORY_QUOTA_PRUNE_ROWS" => Some(prune.to_owned()),
            _ => None,
        })
    };
    let cases = [
        ("default", ActionHistoryConfig::default(), shipped.clone()),
        (
            "no env",
            ActionHistoryConfig::from_lookup(|_| None),
            shipped.clone(),
        ),
        (
            "30 / 100",
            lookup("30", "100"),
            ActionHistoryConfig {
                retention_days: 30,
                quota_prune_rows: 100,
            },
        ),
        ("0 / 0", lookup("0", "0"), shipped.clone()),
        ("abc / x", lookup("abc", "x"), shipped.clone()),
        ("-5 / -1", lookup("-5", "-1"), shipped),
    ];
    for (name, got, expected) in cases {
        if got != expected {
            return Err(format!("{name}: got {got:?}, expected {expected:?}"));
        }
    }
    Ok(())
}

// ---- search -------------------------------------------------------------

fn l17_empty_query_matches_everything() -> Check {
    let row = fixture_row(T);
    check(
        words("").is_empty()
            && words("   ").is_empty()
            && HistoryQuery::new("", None, None, None).matches(&row)
            && HistoryQuery::new("  \t ", None, None, None).matches(&row),
        || "an empty or blank query must match every row".to_owned(),
    )
}

fn l18_every_word_must_appear_case_insensitive() -> Check {
    let row = fixture_row(T);
    check(
        words("NOTEPAD click") == ["notepad", "click"]
            && HistoryQuery::new("NOTEPAD click", None, None, None).matches(&row)
            && !HistoryQuery::new("notepad zebra", None, None, None).matches(&row)
            && !HistoryQuery::new("sess-A", None, None, None).matches(&row),
        || {
            "words across target and summary match; a missing word or a session id does not"
                .to_owned()
        },
    )
}

fn l19_tool_and_operation_fields_are_searched() -> Check {
    let row = ActionHistoryRow {
        record_version: ACTION_HISTORY_RECORD_VERSION,
        ts_ms: T,
        session_id: "sess-B".to_owned(),
        tool: "workspace".to_owned(),
        operation: "ledger_put".to_owned(),
        target: String::new(),
        outcome: "ok".to_owned(),
        summary: "s".to_owned(),
    };
    check(
        HistoryQuery::new("ledger_put", None, None, None).matches(&row)
            && HistoryQuery::new("WORKSPACE", None, None, None).matches(&row)
            && HistoryQuery::new("act invoke", None, None, None).matches(&fixture_row(T)),
        || "tool and operation words must match".to_owned(),
    )
}

fn l20_hostile_query_is_a_literal() -> Check {
    let mut literal = fixture_row(T);
    literal.summary = "a *.( b".to_owned();
    let long = "x".repeat(10_000);
    check(
        !HistoryQuery::new("*.(", None, None, None).matches(&fixture_row(T))
            && HistoryQuery::new("*.(", None, None, None).matches(&literal)
            && !HistoryQuery::new(&long, None, None, None).matches(&fixture_row(T)),
        || "regex characters must compare literally".to_owned(),
    )
}

fn l21_time_window_is_inclusive() -> Check {
    let row = fixture_row(T);
    let cases = [
        (Some(T), Some(T), true),
        (Some(T + 1), None, false),
        (None, Some(T - 1), false),
        (Some(T + 5), Some(T - 5), false),
        (None, None, true),
    ];
    for (since, until, expected) in cases {
        let got = HistoryQuery::new("", since, until, None).matches(&row);
        if got != expected {
            return Err(format!(
                "since {since:?} until {until:?}: got {got}, expected {expected}"
            ));
        }
    }
    Ok(())
}

fn l22_limit_clamp() -> Check {
    let cases = [
        (None, 50),
        (Some(0), 1),
        (Some(1), 1),
        (Some(500), 500),
        (Some(501), 500),
        (Some(usize::MAX), 500),
    ];
    for (limit, expected) in cases {
        let got = HistoryQuery::new("", None, None, limit).limit();
        if got != expected {
            return Err(format!(
                "clamp_search_limit({limit:?}) = {got}, expected {expected}"
            ));
        }
    }
    Ok(())
}

fn l23_newest_first_and_truncated() -> Check {
    let rows = || vec![fixture_row(1), fixture_row(2), fixture_row(3)];
    let times = |rows: &[ActionHistoryRow]| rows.iter().map(|row| row.ts_ms).collect::<Vec<_>>();
    let (two, cut) = HistoryQuery::new("", None, None, Some(2)).newest_first(rows());
    let (all, all_cut) = HistoryQuery::new("", None, None, Some(3)).newest_first(rows());
    let (none, none_cut) = HistoryQuery::new("", None, None, Some(5)).newest_first(Vec::new());
    check(
        times(&two) == [3, 2]
            && cut
            && times(&all) == [3, 2, 1]
            && !all_cut
            && none.is_empty()
            && !none_cut,
        || {
            format!(
                "limit 2 {:?} {cut}, limit 3 {:?} {all_cut}",
                times(&two),
                times(&all)
            )
        },
    )
}

fn l24_operation_from_argument_else_reply() -> Check {
    let cases = [
        (Some("invoke"), None, Some("invoke")),
        (Some(""), Some("invoke"), Some("invoke")),
        (Some("  "), Some("put"), Some("put")),
        (Some("x"), Some("y"), Some("x")),
        (None, None, None),
    ];
    for (argument, reply, expected) in cases {
        let got = resolve_operation(argument, reply);
        if got.as_deref() != expected {
            return Err(format!(
                "resolve_operation({argument:?}, {reply:?}) = {got:?}, expected {expected:?}"
            ));
        }
    }
    Ok(())
}

// ---- second actor, restart, neighbours ----------------------------------

fn l25_two_sessions_same_millisecond_stay_apart() -> Check {
    let args = click_args();
    let mut a = call(APP, TITLE, Some(&args));
    a.session_id = "session-A";
    let mut b = a;
    b.session_id = "session-B";
    let row_a = ActionHistoryRow::build(&a, &same);
    let row_b = ActionHistoryRow::build(&b, &same);
    check(
        fixture_row(T).key(1) != fixture_row(T).key(2)
            && row_a.session_id == "session-A"
            && row_b.session_id == "session-B",
        || format!("keys or sessions mixed: {row_a:?} {row_b:?}"),
    )
}

fn l26_row_survives_encode_decode_only_for_its_version() -> Check {
    let row = fixture_row(T);
    let decoded = row
        .encode()
        .ok()
        .and_then(|bytes| ActionHistoryRow::decode(&bytes));
    let mut other = serde_json::to_value(&row).map_err(|error| error.to_string())?;
    other["record_version"] = json!(2);
    let other_bytes = serde_json::to_vec(&other).map_err(|error| error.to_string())?;
    check(
        decoded.as_ref() == Some(&row)
            && ActionHistoryRow::decode(b"garbage").is_none()
            && ActionHistoryRow::decode(&other_bytes).is_none(),
        || format!("round trip {decoded:?}"),
    )
}

fn l27_history_has_its_own_kv_budget_and_no_prefix_overlap() -> Check {
    let kv_hard_cap = DEFAULTS
        .iter()
        .find(|default| default.cf == "CF_KV")
        .map_or(0, |default| default.hard_cap_mb);
    let own = KV_NAMESPACE_QUOTAS
        .iter()
        .any(|quota| quota.key_prefix == "action-history/v1" && quota.quota_mb > 0);
    let total: u64 = KV_NAMESPACE_QUOTAS.iter().map(|quota| quota.quota_mb).sum();
    let overlap = KV_NAMESPACE_QUOTAS.iter().any(|a| {
        KV_NAMESPACE_QUOTAS
            .iter()
            .any(|b| a.key_prefix != b.key_prefix && a.key_prefix.starts_with(b.key_prefix))
    });
    check(
        own && total < kv_hard_cap
            && !overlap
            && ACTION_HISTORY_PREFIX.starts_with("action-history/v1"),
        || format!("own budget {own}, total {total} MiB vs cap {kv_hard_cap}, overlap {overlap}"),
    )
}

#[test]
fn action_history_row_building() {
    run(rows!(
        l1_normal_row_has_every_field,
        l2_no_target_gives_empty_target_and_no_on,
        l3_partial_target_keeps_the_present_part,
        l4_missing_or_non_object_args_give_bare_summary,
        l5_outcome_label_is_ok_code_or_error,
        l6_summary_boundary_200_kept_201_cut,
        l7_multibyte_cut_on_a_character_boundary,
        l8_control_characters_become_spaces,
        l9_every_text_field_goes_through_redaction,
        l10_redaction_runs_before_the_cut,
        l11_typed_content_never_enters_the_summary,
        l28_child_object_content_never_enters_the_summary,
    ));
}

#[test]
fn action_history_keys_and_retention() {
    run(rows!(
        l12_key_layout,
        l13_keys_sort_by_time_then_sequence,
        l14_extreme_times_keep_the_layout,
        l15_retention_cutoff,
        l16_config_default_and_overrides,
    ));
}

#[test]
fn action_history_search() {
    run(rows!(
        l17_empty_query_matches_everything,
        l18_every_word_must_appear_case_insensitive,
        l19_tool_and_operation_fields_are_searched,
        l20_hostile_query_is_a_literal,
        l21_time_window_is_inclusive,
        l22_limit_clamp,
        l23_newest_first_and_truncated,
        l24_operation_from_argument_else_reply,
    ));
}

#[test]
fn action_history_neighbours() {
    run(rows!(
        l25_two_sessions_same_millisecond_stay_apart,
        l26_row_survives_encode_decode_only_for_its_version,
        l27_history_has_its_own_kv_budget_and_no_prefix_overlap,
    ));
}
