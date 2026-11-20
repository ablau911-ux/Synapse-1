//! #2326 step B frozen behaviour table (manager-owned, work method v3).
//! Codex and Opus never edit this file. One case per L row of the step-B
//! table (route memory wired into `act`): the pure adapters between the
//! click route and the route-memory core in `synapse_core::route_memory`.
//!
//! Rows that expect "no change" carry a contrast check (the same call where
//! memory must act), so they fail on a stub that does nothing.

use serde_json::{Value, json};
use synapse_core::UiaPattern;
use synapse_core::route_memory::{
    ControlIdentity, ExeStamp, OutcomeKind, ROUTE_MEMORY_PREFIX, RouteMemoryConfig, RoutePlan,
    TierOutcome, apply_outcome, control_signature, encode_row, exe_key_prefix, route_memory_key,
};

use super::{
    ClickRouteFacts, ROUTE_MEMORY_STATUS_LIMIT, RouteMemoryReply, click_coordinate_first,
    click_static_tier_order, outcomes_from_click_result, route_memory_reply, route_memory_scope,
    status_readback,
};

fn verified(tier: &str) -> TierOutcome {
    TierOutcome {
        tier: tier.to_owned(),
        kind: OutcomeKind::Verified,
    }
}

fn failed(tier: &str, reason: &str) -> TierOutcome {
    TierOutcome {
        tier: tier.to_owned(),
        kind: OutcomeKind::Failed {
            reason: reason.to_owned(),
        },
    }
}

fn refused(tier: &str) -> TierOutcome {
    TierOutcome {
        tier: tier.to_owned(),
        kind: OutcomeKind::Refused,
    }
}

fn unverified(tier: &str) -> TierOutcome {
    TierOutcome {
        tier: tier.to_owned(),
        kind: OutcomeKind::Unverified,
    }
}

fn attempt(tier: &str, status: &str, reason: Option<&str>) -> Value {
    json!({
        "tier": tier,
        "status": status,
        "reason_code": reason,
        "error_code": null,
        "detail": null,
        "required_foreground": tier == "foreground",
    })
}

fn ok_result(attempts: Vec<Value>, verified: bool) -> Value {
    let postcondition = if verified {
        json!({"status": "observed_delta", "observed_delta": true})
    } else {
        json!({"status": "not_requested"})
    };
    json!({"ok": true, "tier_attempts": attempts, "postcondition": postcondition})
}

fn error_result(attempts: Vec<Value>) -> Value {
    json!({
        "error": {
            "delegated_tool": "act_click",
            "code": "ACTION_NO_OBSERVED_DELTA",
            "message": "no observed delta",
            "data": {"tier_attempts": attempts},
        }
    })
}

fn report(table: &str, failures: &[String]) {
    assert!(
        failures.is_empty(),
        "#2326 step B {table} rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn outcomes_from_click_result_table_2326() {
    let rows: Vec<(&str, &str, Value, Vec<TierOutcome>)> = vec![
        (
            "o01_normal_uia_delivered_and_verified",
            "delivered_and_verified",
            ok_result(vec![attempt("uia", "delivered", None)], true),
            vec![verified("uia")],
        ),
        (
            "o02_uia_pattern_unsupported_then_postmessage_verified",
            "delivered_and_verified",
            ok_result(
                vec![
                    attempt("uia", "failed", Some("pattern_unsupported")),
                    attempt("postmessage", "delivered", None),
                ],
                true,
            ),
            vec![
                failed("uia", "pattern_unsupported"),
                verified("postmessage"),
            ],
        ),
        (
            "o03_delivered_no_observed_delta_is_failed_on_delivering_tier",
            "delivered_unverified",
            error_result(vec![
                attempt("uia", "delivered", None),
                attempt("uia", "failed", Some("no_observed_delta")),
            ]),
            vec![failed("uia", "no_observed_delta")],
        ),
        (
            "o04_refused_before_delivery_counts_no_ok_or_fail",
            "refused_before_delivery",
            error_result(vec![
                attempt("uia", "failed", Some("pattern_unsupported")),
                attempt("foreground", "failed", Some("foreground_refused")),
            ]),
            vec![refused("uia"), refused("foreground")],
        ),
        (
            "o05_deferred_counts_no_ok_or_fail",
            "deferred_without_delivery",
            error_result(vec![attempt(
                "postmessage",
                "failed",
                Some("backend_unavailable"),
            )]),
            vec![refused("postmessage")],
        ),
        (
            "o06_delivered_without_verification_is_unverified",
            "delivered_unverified",
            ok_result(vec![attempt("postmessage", "delivered", None)], false),
            vec![unverified("postmessage")],
        ),
        (
            "o07_pixel_verified_is_verified",
            "delivered_and_pixel_verified",
            ok_result(vec![attempt("postmessage", "delivered", None)], true),
            vec![verified("postmessage")],
        ),
        (
            "o08_failed_before_delivery_capability_failures_are_failed",
            "failed_before_delivery",
            error_result(vec![
                attempt("uia", "failed", Some("pattern_unsupported")),
                attempt("postmessage", "failed", Some("backend_unavailable")),
            ]),
            vec![
                failed("uia", "pattern_unsupported"),
                failed("postmessage", "backend_unavailable"),
            ],
        ),
        (
            "o09_delivery_unknown_is_unverified_and_stale_element_is_refused",
            "failed_before_delivery",
            error_result(vec![
                attempt("uia", "failed", Some("element_stale")),
                attempt("postmessage", "failed", Some("delivery_unknown")),
            ]),
            vec![refused("uia"), unverified("postmessage")],
        ),
        (
            "o10_hostile_unknown_tier_and_malformed_attempts_are_dropped",
            "delivered_and_verified",
            ok_result(
                vec![
                    attempt("kernel_inject", "delivered", None),
                    json!("not an object"),
                    json!({"tier": "uia"}),
                    attempt("uia", "exploded", None),
                    attempt("uia", "delivered", None),
                ],
                true,
            ),
            vec![verified("uia")],
        ),
        (
            "o11_hostile_attempt_flood_gives_one_outcome_per_tier",
            "delivered_unverified",
            error_result(
                (0..1000)
                    .map(|_| attempt("uia", "failed", Some("no_observed_delta")))
                    .collect(),
            ),
            vec![failed("uia", "no_observed_delta")],
        ),
    ];
    let mut failures = Vec::new();
    for (row, delivery_state, result, expected) in rows {
        let got = outcomes_from_click_result(delivery_state, &result);
        if got != expected {
            failures.push(format!("row {row}: got {got:?}, expected {expected:?}"));
        }
    }

    // o12: no attempts -> no outcome; contrast: o01's input gives one.
    let row = "o12_empty_or_missing_attempts_give_no_outcome";
    for empty in [
        json!({}),
        Value::Null,
        json!({"tier_attempts": "not a list"}),
        ok_result(Vec::new(), true),
    ] {
        let got = outcomes_from_click_result("delivered_and_verified", &empty);
        if !got.is_empty() {
            failures.push(format!("row {row}: {empty} gave {got:?}, expected none"));
        }
    }
    let contrast = outcomes_from_click_result(
        "delivered_and_verified",
        &ok_result(vec![attempt("uia", "delivered", None)], true),
    );
    if contrast != vec![verified("uia")] {
        failures.push(format!(
            "row {row}: contrast (one verified uia attempt) gave {contrast:?}"
        ));
    }
    report("outcomes", &failures);
}

fn facts<'a>(
    web_element: bool,
    coordinate_only: bool,
    use_invoke_pattern: bool,
    coordinate_fallback_on_unsupported: bool,
    role: &'a str,
    patterns: &'a [UiaPattern],
) -> ClickRouteFacts<'a> {
    ClickRouteFacts {
        web_element,
        coordinate_only,
        use_invoke_pattern,
        coordinate_fallback_on_unsupported,
        role,
        patterns,
    }
}

#[test]
fn click_static_tier_order_table_2326() {
    let invoke = [UiaPattern::Invoke];
    let value = [UiaPattern::Value];
    let text = [UiaPattern::Text];
    let none: [UiaPattern; 0] = [];
    let rows: Vec<(&str, ClickRouteFacts<'_>, Vec<&str>)> = vec![
        (
            "s01_native_button_is_uia_then_postmessage_then_foreground",
            facts(false, false, true, true, "button", &invoke),
            vec!["uia", "postmessage", "foreground"],
        ),
        (
            "s02_web_element_has_one_tier",
            facts(true, false, true, true, "button", &invoke),
            vec!["cdp"],
        ),
        (
            "s03_coordinate_only_element_is_postmessage_then_foreground",
            facts(false, true, true, true, "", &none),
            vec!["postmessage", "foreground"],
        ),
        (
            "s04_editable_role_needs_foreground_caret_no_postmessage",
            facts(false, false, true, true, "edit", &none),
            vec!["uia", "foreground"],
        ),
        (
            "s05_value_pattern_needs_foreground_caret_no_postmessage",
            facts(false, false, true, true, "button", &value),
            vec!["uia", "foreground"],
        ),
        (
            "s06_document_role_or_text_pattern_is_caret",
            facts(false, false, true, true, "Document", &text),
            vec!["uia", "foreground"],
        ),
        (
            "s07_list_item_goes_coordinate_direct",
            facts(false, false, true, true, "list item", &none),
            vec!["postmessage", "foreground"],
        ),
        (
            "s08_editable_list_item_is_foreground_only",
            facts(false, false, true, true, "list item", &value),
            vec!["foreground"],
        ),
        (
            "s09_invoke_pattern_off_goes_coordinate_direct",
            facts(false, false, false, true, "button", &invoke),
            vec!["postmessage", "foreground"],
        ),
        (
            "s10_coordinate_fallback_off_is_uia_only",
            facts(false, false, true, false, "button", &invoke),
            vec!["uia"],
        ),
        (
            "s11_hostile_empty_role_and_no_patterns_is_full_native_order",
            facts(false, false, true, true, "", &none),
            vec!["uia", "postmessage", "foreground"],
        ),
        (
            "s12_role_is_matched_case_insensitively",
            facts(false, false, true, true, "EDIT", &none),
            vec!["uia", "foreground"],
        ),
    ];
    let mut failures = Vec::new();
    for (row, input, expected) in rows {
        let got = click_static_tier_order(&input);
        if got != expected {
            failures.push(format!("row {row}: got {got:?}, expected {expected:?}"));
        }
    }
    report("static order", &failures);
}

#[test]
fn click_coordinate_first_table_2326() {
    let full = ["uia", "postmessage", "foreground"];
    let mut failures = Vec::new();
    let promoted = click_coordinate_first(&full, &["postmessage", "uia", "foreground"]);
    if !promoted {
        failures.push("row h01_postmessage_promoted_over_uia_sets_hint: got false".to_owned());
    }
    // (row, static order, planned order); every row expects false and is
    // contrasted with h01.
    let rows: [(&str, &[&str], &[&str]); 5] = [
        ("h02_unchanged_order_sets_no_hint", &full, &full),
        (
            "h03_foreground_first_never_sets_hint",
            &full,
            &["foreground", "uia", "postmessage"],
        ),
        (
            "h04_static_order_without_uia_never_sets_hint",
            &["postmessage", "foreground"],
            &["postmessage", "foreground"],
        ),
        ("h05_empty_orders_set_no_hint", &[], &[]),
        (
            "h06_foreground_over_uia_without_postmessage_sets_no_hint",
            &["uia", "foreground"],
            &["foreground", "uia"],
        ),
    ];
    for (row, static_order, plan_order) in rows {
        if click_coordinate_first(static_order, plan_order) {
            failures.push(format!("row {row}: got true, expected false"));
        }
        if !promoted {
            failures.push(format!("row {row}: contrast h01 did not set the hint"));
        }
    }
    report("coordinate-first hint", &failures);
}

#[test]
fn route_memory_reply_table_2326() {
    let key = "route-memory/v1/notepad.exe/click/0123456789abcdef";
    let used = RoutePlan {
        order: vec!["postmessage", "uia", "foreground"],
        used: true,
        tier: Some("postmessage"),
        evidence_count: 5,
        confidence: 0.6,
    };
    let unused = RoutePlan {
        order: vec!["uia", "postmessage", "foreground"],
        used: false,
        tier: None,
        evidence_count: 0,
        confidence: 0.0,
    };
    let source_of_truth = format!("CF_KV {key}");
    let rows: Vec<(&str, bool, bool, &RoutePlan<'_>, Option<RouteMemoryReply>)> = vec![
        (
            "p01_used_plan_reports_tier_evidence_confidence_and_key",
            true,
            true,
            &used,
            Some(RouteMemoryReply {
                used: true,
                tier: Some("postmessage".to_owned()),
                evidence_count: 5,
                confidence: 0.6,
                source_of_truth: source_of_truth.clone(),
            }),
        ),
        (
            "p02_row_present_but_unused_reports_used_false",
            true,
            true,
            &unused,
            Some(RouteMemoryReply {
                used: false,
                tier: None,
                evidence_count: 0,
                confidence: 0.0,
                source_of_truth: source_of_truth.clone(),
            }),
        ),
        ("p03_memory_off_gives_no_reply", false, true, &used, None),
        ("p04_no_row_gives_no_reply", true, false, &used, None),
    ];
    let mut failures = Vec::new();
    for (row, enabled, row_present, plan, expected) in rows {
        let got = route_memory_reply(enabled, row_present, plan, key);
        if got != expected {
            failures.push(format!("row {row}: got {got:?}, expected {expected:?}"));
        }
        if expected.is_none() && route_memory_reply(true, true, plan, key).is_none() {
            failures.push(format!(
                "row {row}: contrast (memory on, row present) gave no reply"
            ));
        }
    }
    report("reply", &failures);
}

#[test]
fn route_memory_scope_table_2326() {
    let mut failures = Vec::new();
    let whole = route_memory_scope(None);
    if whole.as_deref() != Ok(ROUTE_MEMORY_PREFIX) {
        failures.push(format!(
            "row c01_no_exe_is_whole_namespace: got {whole:?}, expected Ok({ROUTE_MEMORY_PREFIX:?})"
        ));
    }
    let app = route_memory_scope(Some("Notepad.EXE"));
    if app.as_deref() != Ok("route-memory/v1/notepad.exe/")
        || app.as_deref() != Ok(exe_key_prefix("notepad.exe").as_str())
    {
        failures.push(format!(
            "row c02_exe_is_its_lowercased_app_prefix: got {app:?}"
        ));
    }
    for blank in ["", "   "] {
        let got = route_memory_scope(Some(blank));
        if got.is_ok() {
            failures.push(format!(
                "row c03_blank_exe_is_rejected_never_widened: {blank:?} gave {got:?}"
            ));
        }
    }
    if whole.is_err() {
        failures.push(
            "row c03_blank_exe_is_rejected_never_widened: contrast (no exe) was rejected"
                .to_owned(),
        );
    }
    match route_memory_scope(Some("..\\X/y.EXE")) {
        Ok(prefix) => {
            let rest = prefix.strip_prefix(ROUTE_MEMORY_PREFIX).unwrap_or("/");
            if rest.contains("..") || rest.matches('/').count() != 1 || !rest.ends_with('/') {
                failures.push(format!(
                    "row c04_hostile_exe_stays_inside_namespace: got {prefix:?}"
                ));
            }
        }
        Err(error) => failures.push(format!(
            "row c04_hostile_exe_stays_inside_namespace: rejected ({error})"
        )),
    }
    let other_key = control_signature(&ControlIdentity {
        exe: "notepad.exe2",
        window_class: "",
        role: "button",
        automation_id: "Save",
        name: "",
    })
    .map(|sig| route_memory_key(&sig, "click"));
    match (route_memory_scope(Some("notepad.exe")), other_key) {
        (Ok(prefix), Some(other_key)) => {
            if other_key.starts_with(&prefix) || exe_key_prefix("notepad.exe2").starts_with(&prefix)
            {
                failures.push(format!(
                    "row c05_someone_elses_app_is_outside_the_scope: {prefix:?} covers {other_key:?}"
                ));
            }
        }
        (scope, key) => failures.push(format!(
            "row c05_someone_elses_app_is_outside_the_scope: scope {scope:?}, key {key:?}"
        )),
    }
    report("scope", &failures);
}

fn stored_row(
    exe: &str,
    automation_id: &str,
    name: &str,
    outcome: TierOutcome,
) -> (Vec<u8>, Vec<u8>) {
    let sig = control_signature(&ControlIdentity {
        exe,
        window_class: "",
        role: "button",
        automation_id,
        name,
    });
    let Some(sig) = sig else {
        return (Vec::new(), Vec::new());
    };
    let stats = apply_outcome(
        None,
        &sig,
        &[outcome],
        1_000,
        ExeStamp::default(),
        &RouteMemoryConfig::default(),
    );
    (
        route_memory_key(&sig, "click").into_bytes(),
        encode_row(&stats).unwrap_or_default(),
    )
}

#[test]
fn status_readback_table_2326() {
    let prefix = exe_key_prefix("notepad.exe");
    let mut failures = Vec::new();

    let two = vec![
        stored_row("notepad.exe", "Save", "", verified("uia")),
        stored_row(
            "notepad.exe",
            "Open",
            "",
            failed("uia", "no_observed_delta"),
        ),
    ];
    let got = status_readback(&prefix, two.clone(), true);
    let uia_counts: Vec<(u64, u64)> = got
        .rows
        .iter()
        .flat_map(|row| row.tiers.iter())
        .filter(|tier| tier.tier == "uia")
        .map(|tier| (tier.ok, tier.fail))
        .collect();
    let mut sorted_counts = uia_counts.clone();
    sorted_counts.sort_unstable();
    if got.rows.len() != 2
        || got.total_rows != 2
        || got.corrupt_rows != 0
        || got.truncated
        || sorted_counts != vec![(0, 1), (1, 0)]
        || got.prefix != prefix
        || got.source_of_truth != format!("CF_KV {prefix}")
    {
        failures.push(format!(
            "row t01_valid_rows_are_listed_with_counts: got {got:?}"
        ));
    }

    let mut with_corrupt = two.clone();
    with_corrupt.push((
        format!("{prefix}click/ffffffffffffffff").into_bytes(),
        b"{oops".to_vec(),
    ));
    let got = status_readback(&prefix, with_corrupt, true);
    if got.rows.len() != 2 || got.total_rows != 3 || got.corrupt_rows != 1 {
        failures.push(format!(
            "row t02_corrupt_row_is_counted_not_listed: got rows={} total={} corrupt={}",
            got.rows.len(),
            got.total_rows,
            got.corrupt_rows
        ));
    }

    let many: Vec<(Vec<u8>, Vec<u8>)> = (0..=ROUTE_MEMORY_STATUS_LIMIT)
        .map(|index| {
            stored_row(
                "notepad.exe",
                &format!("Button{index}"),
                "",
                verified("uia"),
            )
        })
        .collect();
    let total = u64::try_from(many.len()).unwrap_or(u64::MAX);
    let got = status_readback(&prefix, many, true);
    if got.rows.len() != ROUTE_MEMORY_STATUS_LIMIT || !got.truncated || got.total_rows != total {
        failures.push(format!(
            "row t03_over_limit_is_truncated_with_true_total: got rows={} truncated={} total={}",
            got.rows.len(),
            got.truncated,
            got.total_rows
        ));
    }

    let got = status_readback(&prefix, Vec::new(), true);
    let contrast = status_readback(&prefix, two, true);
    if !got.rows.is_empty() || got.total_rows != 0 || got.truncated || contrast.rows.len() != 2 {
        failures.push(format!(
            "row t04_empty_namespace_is_empty: got {got:?}; contrast rows={}",
            contrast.rows.len()
        ));
    }

    let private = vec![stored_row(
        "notepad.exe",
        "",
        "Secret Customer 42",
        verified("uia"),
    )];
    let got = status_readback(&prefix, private, true);
    let text = serde_json::to_string(&got).unwrap_or_default();
    if got.rows.len() != 1 || text.contains("Secret") || text.contains("Customer") {
        failures.push(format!(
            "row t05_listing_never_shows_a_raw_name: got {text}"
        ));
    }
    report("status", &failures);
}
