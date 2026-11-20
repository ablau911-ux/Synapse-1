//! Frozen behaviour table for issue #2326 rows 1-25 (route memory pure core).
//! Written by the manager from the table in Gitea synapse/synapse #2326 (work method v3).
//! Code agents must not edit this file.
//!
//! One test per function group, one case per row, each case named after its row.
//! Failures are collected per row so one run shows every failing row.
//! Rows 2, 3, 7, 10, 11, 15, 16 and 23 carry a contrast check (the same call where
//! memory must act) so they cannot pass on a build that has no memory at all.

use std::collections::BTreeMap;

use crate::route_memory::{
    ControlIdentity, ControlRouteStats, ControlSignature, ExeStamp, Freshness, IdKind, OutcomeKind,
    PruneCandidate, ROUTE_MEMORY_KEY_MAX_LEN, ROUTE_MEMORY_PREFIX, ROUTE_MEMORY_RECORD_VERSION,
    RouteMemoryConfig, RoutePlan, TierOutcome, TierStats, apply_outcome, choose_tier_order,
    control_signature, decode_row, encode_row, exe_key_prefix, prune_plan, route_memory_key,
    stats_freshness,
};
use crate::routines::wilson_lower_bound;

type Check = Result<(), String>;

const NOW: u64 = 1_800_000_000_000_000_000;
const NS_PER_DAY: u64 = 86_400_000_000_000;
const STAMP: ExeStamp = ExeStamp {
    size: 4_096,
    mtime_ns: 1_700_000_000_000_000_000,
};
const CLICK_ORDER: [&str; 3] = ["uia", "postmessage", "foreground"];
const PROMOTED: [&str; 3] = ["postmessage", "uia", "foreground"];

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
        "{} row(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn ensure(holds: bool, what: impl Into<String>) -> Check {
    if holds { Ok(()) } else { Err(what.into()) }
}

// ---------------------------------------------------------------- fixtures

/// Explicit thresholds so the table does not depend on default tuning.
fn table_cfg() -> RouteMemoryConfig {
    RouteMemoryConfig {
        enabled: true,
        min_evidence: 5,
        min_confidence: 0.5,
        min_margin: 0.2,
        max_fail_streak: 3,
        stale_after_days: 30,
        count_cap: 1_000,
        max_rows_per_app: 512,
        future_skew_ns: 300_000_000_000,
        allow_foreground_promotion: false,
    }
}

fn sig() -> ControlSignature {
    ControlSignature {
        exe: "notepad.exe".to_owned(),
        window_class: "Notepad".to_owned(),
        role: "button".to_owned(),
        id_kind: IdKind::AutomationId,
        id: "SaveButton".to_owned(),
    }
}

fn tier(ok: u64, fail: u64, fail_streak: u32) -> TierStats {
    TierStats {
        ok,
        fail,
        fail_streak,
        last_ts_ns: NOW,
        ..TierStats::default()
    }
}

fn stats(tiers: &[(&str, TierStats)]) -> ControlRouteStats {
    ControlRouteStats {
        record_version: ROUTE_MEMORY_RECORD_VERSION,
        ts_ns: NOW,
        first_ts_ns: NOW,
        sig: sig(),
        exe_stamp: STAMP,
        tiers: tiers
            .iter()
            .map(|(name, stats)| ((*name).to_owned(), stats.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// Row 1's evidence: `uia` failed three times, `postmessage` worked five times.
fn row_one_stats() -> ControlRouteStats {
    stats(&[("uia", tier(0, 3, 3)), ("postmessage", tier(5, 0, 0))])
}

fn plan<'a>(
    order: &[&'a str],
    row: Option<&ControlRouteStats>,
    cfg: &RouteMemoryConfig,
) -> RoutePlan<'a> {
    choose_tier_order(order, row, NOW, STAMP, cfg)
}

fn outcome(tier: &str, kind: OutcomeKind) -> TierOutcome {
    TierOutcome {
        tier: tier.to_owned(),
        kind,
    }
}

fn failed(reason: &str) -> OutcomeKind {
    OutcomeKind::Failed {
        reason: reason.to_owned(),
    }
}

fn apply(
    prior: Option<ControlRouteStats>,
    outcomes: &[TierOutcome],
    cfg: &RouteMemoryConfig,
) -> ControlRouteStats {
    apply_outcome(prior, &sig(), outcomes, NOW + 1, STAMP, cfg)
}

fn tier_of(row: &ControlRouteStats, name: &str) -> TierStats {
    row.tiers.get(name).cloned().unwrap_or_default()
}

const fn counts(tier: &TierStats) -> (u64, u64) {
    (tier.ok, tier.fail)
}

fn encoded_text(row: &ControlRouteStats) -> Result<String, String> {
    let bytes = encode_row(row).map_err(|error| format!("encode failed: {error}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ------------------------------------------------- choose_tier_order rows

fn row_01_uia_no_delta_postmessage_ok_promotes_postmessage() -> Check {
    let row = row_one_stats();
    let p = plan(&CLICK_ORDER, Some(&row), &table_cfg());
    ensure(p.order == PROMOTED, format!("order {:?}", p.order))?;
    ensure(p.used, "used=false")?;
    ensure(p.tier == Some("postmessage"), format!("tier {:?}", p.tier))?;
    ensure(
        p.evidence_count == 5,
        format!("evidence {}", p.evidence_count),
    )
}

fn row_02_no_row_keeps_static_order() -> Check {
    let p = plan(&CLICK_ORDER, None, &table_cfg());
    ensure(
        p.order == CLICK_ORDER && !p.used && p.tier.is_none() && p.evidence_count == 0,
        format!("no row gave {p:?}"),
    )?;
    let row = row_one_stats();
    ensure(
        plan(&CLICK_ORDER, Some(&row), &table_cfg()).used,
        "contrast: the same call with row 1's evidence did not use memory",
    )
}

fn row_03_disabled_keeps_static_order() -> Check {
    let row = row_one_stats();
    let off = RouteMemoryConfig {
        enabled: false,
        ..table_cfg()
    };
    let p = plan(&CLICK_ORDER, Some(&row), &off);
    ensure(
        p.order == CLICK_ORDER && !p.used,
        format!("disabled gave {p:?}"),
    )?;
    ensure(
        plan(&CLICK_ORDER, Some(&row), &table_cfg()).used,
        "contrast: enabled with the same stats did not promote",
    )
}

fn row_04_min_evidence_boundary() -> Check {
    let cfg = RouteMemoryConfig {
        min_confidence: 0.0,
        min_margin: 0.0,
        ..table_cfg()
    };
    for (evidence, promote) in [(cfg.min_evidence - 1, false), (cfg.min_evidence, true)] {
        let row = stats(&[
            ("uia", tier(0, 3, 3)),
            ("postmessage", tier(evidence, 0, 0)),
        ]);
        let p = plan(&CLICK_ORDER, Some(&row), &cfg);
        let want: &[&str] = if promote { &PROMOTED } else { &CLICK_ORDER };
        ensure(
            p.order == want && p.used == promote,
            format!("evidence {evidence}: order {:?} used {}", p.order, p.used),
        )?;
    }
    Ok(())
}

fn row_05_confidence_and_margin_boundaries() -> Check {
    let row = stats(&[("uia", tier(1, 3, 0)), ("postmessage", tier(5, 0, 0))]);
    let later = wilson_lower_bound(5, 5);
    let margin = later - wilson_lower_bound(1, 4);
    let cases = [
        (
            "confidence just below",
            RouteMemoryConfig {
                min_confidence: later + 1e-9,
                min_margin: 0.0,
                ..table_cfg()
            },
            false,
        ),
        (
            "confidence exactly at",
            RouteMemoryConfig {
                min_confidence: later,
                min_margin: 0.0,
                ..table_cfg()
            },
            true,
        ),
        (
            "margin just short",
            RouteMemoryConfig {
                min_confidence: 0.0,
                min_margin: margin + 1e-9,
                ..table_cfg()
            },
            false,
        ),
        (
            "margin exactly at",
            RouteMemoryConfig {
                min_confidence: 0.0,
                min_margin: margin,
                ..table_cfg()
            },
            true,
        ),
    ];
    for (label, cfg, promote) in cases {
        let p = plan(&CLICK_ORDER, Some(&row), &cfg);
        let want: &[&str] = if promote { &PROMOTED } else { &CLICK_ORDER };
        ensure(
            p.order == want && p.used == promote,
            format!("{label}: order {:?} used {}", p.order, p.used),
        )?;
    }
    Ok(())
}

fn row_06_fail_streak_at_limit_blocks_promotion() -> Check {
    let cfg = table_cfg();
    let max = cfg.max_fail_streak;
    for (streak, promote) in [(max - 1, true), (max, false)] {
        let row = stats(&[("uia", tier(0, 3, 3)), ("postmessage", tier(10, 0, streak))]);
        let p = plan(&CLICK_ORDER, Some(&row), &cfg);
        let want: &[&str] = if promote { &PROMOTED } else { &CLICK_ORDER };
        ensure(
            p.order == want && p.used == promote,
            format!("streak {streak}: order {:?} used {}", p.order, p.used),
        )?;
    }
    Ok(())
}

fn row_07_earlier_tier_without_failures_is_not_overtaken() -> Check {
    let cfg = table_cfg();
    for earlier in [None, Some(tier(4, 0, 0))] {
        let mut tiers = vec![("postmessage", tier(10, 0, 0))];
        if let Some(uia) = earlier.clone() {
            tiers.push(("uia", uia));
        }
        let row = stats(&tiers);
        let p = plan(&CLICK_ORDER, Some(&row), &cfg);
        ensure(
            p.order == CLICK_ORDER && !p.used,
            format!("earlier {earlier:?}: order {:?}", p.order),
        )?;
    }
    let row = stats(&[("uia", tier(4, 1, 0)), ("postmessage", tier(10, 0, 0))]);
    ensure(
        plan(&CLICK_ORDER, Some(&row), &cfg).order == PROMOTED,
        "contrast: with failure evidence on uia the good later tier was not promoted",
    )
}

fn row_15_foreground_absent_from_static_order_is_never_added() -> Check {
    let row = stats(&[
        ("uia", tier(0, 3, 3)),
        ("postmessage", tier(5, 0, 0)),
        ("foreground", tier(100, 0, 0)),
    ]);
    let p = plan(&["uia", "postmessage"], Some(&row), &table_cfg());
    ensure(
        !p.order.contains(&"foreground"),
        format!("foreground added: {:?}", p.order),
    )?;
    ensure(
        p.order == ["postmessage", "uia"],
        format!("memory did not reorder the allowed tiers: {:?}", p.order),
    )?;
    let p = plan(&CLICK_ORDER, Some(&row), &table_cfg());
    ensure(
        p.order == PROMOTED,
        format!("foreground left its last place: {:?}", p.order),
    )
}

fn row_16_order_is_a_permutation_and_foreground_never_moves_earlier() -> Check {
    const GRID: [(u64, u64); 6] = [(0, 0), (0, 3), (2, 2), (5, 0), (10, 1), (40, 0)];
    let orders: [[&str; 3]; 2] = [CLICK_ORDER, ["uia", "win32_message", "foreground"]];
    let cfg = table_cfg();
    let mut reordered = 0_u32;
    for order in &orders {
        for a in GRID {
            for b in GRID {
                for f in GRID {
                    let row = stats(&[
                        (order[0], tier(a.0, a.1, 0)),
                        (order[1], tier(b.0, b.1, 0)),
                        (order[2], tier(f.0, f.1, 0)),
                    ]);
                    let p = plan(order, Some(&row), &cfg);
                    let mut got = p.order.clone();
                    got.sort_unstable();
                    let mut want = order.to_vec();
                    want.sort_unstable();
                    ensure(
                        got == want,
                        format!(
                            "{order:?} {a:?} {b:?} {f:?}: not a permutation {:?}",
                            p.order
                        ),
                    )?;
                    ensure(
                        p.order.iter().position(|t| *t == "foreground") == Some(2),
                        format!(
                            "{order:?} {a:?} {b:?} {f:?}: foreground moved {:?}",
                            p.order
                        ),
                    )?;
                    if p.order.as_slice() != order.as_slice() {
                        reordered += 1;
                    }
                }
            }
        }
    }
    ensure(
        reordered > 0,
        "contrast: no stats in the grid changed the order, so the property was vacuous",
    )
}

// ---------------------------------------------------- stats_freshness rows

fn row_21_stale_binary_and_future_rows_are_ignored() -> Check {
    let cfg = table_cfg();
    let age_limit = cfg.stale_after_days * NS_PER_DAY;
    let fresh = row_one_stats();
    let mut at_limit = row_one_stats();
    at_limit.ts_ns = NOW - age_limit;
    let mut too_old = row_one_stats();
    too_old.ts_ns = NOW - age_limit - 1;
    let mut future = row_one_stats();
    future.ts_ns = NOW + cfg.future_skew_ns + 1;
    let other_binary = ExeStamp {
        size: STAMP.size + 1,
        ..STAMP
    };
    let cases = [
        (
            "age exactly at the limit",
            &at_limit,
            STAMP,
            Freshness::Fresh,
        ),
        ("older than the limit", &too_old, STAMP, Freshness::StaleAge),
        (
            "exe stamp differs",
            &fresh,
            other_binary,
            Freshness::StaleBinary,
        ),
        (
            "ts beyond the clock skew",
            &future,
            STAMP,
            Freshness::FutureClock,
        ),
    ];
    for (label, row, stamp, want) in cases {
        let got = stats_freshness(row, NOW, stamp, &cfg);
        ensure(
            got == want,
            format!("{label}: freshness {got:?}, want {want:?}"),
        )?;
        if want != Freshness::Fresh {
            let p = choose_tier_order(&CLICK_ORDER, Some(row), NOW, stamp, &cfg);
            ensure(
                p.order == CLICK_ORDER && !p.used,
                format!("{label}: stale row still reordered {:?}", p.order),
            )?;
        }
    }
    ensure(
        choose_tier_order(&CLICK_ORDER, Some(&fresh), NOW, STAMP, &cfg).used,
        "contrast: the fresh row did not promote",
    )
}

// ------------------------------------------------------ apply_outcome rows

fn row_08_verified_counts_ok_and_resets_streak() -> Check {
    let prior = stats(&[("uia", tier(1, 2, 2)), ("postmessage", tier(0, 1, 1))]);
    let after = apply(
        Some(prior.clone()),
        &[outcome("uia", OutcomeKind::Verified)],
        &table_cfg(),
    );
    let uia = tier_of(&after, "uia");
    ensure(
        uia.ok == 2 && uia.fail == 2 && uia.fail_streak == 0,
        format!("uia {uia:?}"),
    )?;
    ensure(
        after.tiers.get("postmessage") == prior.tiers.get("postmessage"),
        "postmessage changed",
    )
}

fn row_09_no_observed_delta_counts_fail_and_reason() -> Check {
    let prior = stats(&[("uia", tier(1, 2, 2)), ("postmessage", tier(0, 1, 1))]);
    let after = apply(
        Some(prior.clone()),
        &[outcome("postmessage", failed("no_observed_delta"))],
        &table_cfg(),
    );
    let pm = tier_of(&after, "postmessage");
    ensure(
        pm.ok == 0
            && pm.fail == 2
            && pm.fail_streak == 2
            && pm.last_reason.as_deref() == Some("no_observed_delta"),
        format!("postmessage {pm:?}"),
    )?;
    ensure(
        after.tiers.get("uia") == prior.tiers.get("uia"),
        "uia changed",
    )
}

fn row_10_refused_or_deferred_changes_no_ok_fail() -> Check {
    let cfg = table_cfg();
    let prior = stats(&[("uia", tier(1, 2, 2)), ("postmessage", tier(0, 1, 1))]);
    for (label, outcomes) in [
        ("refused", vec![outcome("uia", OutcomeKind::Refused)]),
        ("deferred", Vec::new()),
    ] {
        let after = apply(Some(prior.clone()), &outcomes, &cfg);
        for name in ["uia", "postmessage", "foreground"] {
            ensure(
                counts(&tier_of(&after, name)) == counts(&tier_of(&prior, name)),
                format!(
                    "{label}: {name} ok/fail changed to {:?}",
                    tier_of(&after, name)
                ),
            )?;
        }
    }
    let after = apply(Some(prior), &[outcome("uia", OutcomeKind::Refused)], &cfg);
    ensure(
        tier_of(&after, "uia").refused == 1,
        "contrast: the refusal itself was not counted",
    )
}

fn row_11_delivered_unverified_changes_no_ok_fail() -> Check {
    let prior = stats(&[("uia", tier(1, 2, 2))]);
    let after = apply(
        Some(prior.clone()),
        &[outcome("uia", OutcomeKind::Unverified)],
        &table_cfg(),
    );
    let uia = tier_of(&after, "uia");
    ensure(
        counts(&uia) == counts(&tier_of(&prior, "uia")),
        format!("ok/fail changed: {uia:?}"),
    )?;
    ensure(
        uia.unverified == 1,
        "contrast: the unverified delivery was not counted",
    )
}

fn row_12_two_attempts_count_on_their_own_tiers() -> Check {
    let after = apply(
        None,
        &[
            outcome("uia", failed("pattern_unsupported")),
            outcome("postmessage", OutcomeKind::Verified),
        ],
        &table_cfg(),
    );
    let uia = tier_of(&after, "uia");
    let pm = tier_of(&after, "postmessage");
    ensure(counts(&uia) == (0, 1), format!("uia {uia:?}"))?;
    ensure(counts(&pm) == (1, 0), format!("postmessage {pm:?}"))
}

fn row_22_count_cap_halves_and_keeps_ratio() -> Check {
    let cfg = RouteMemoryConfig {
        count_cap: 100,
        ..table_cfg()
    };
    let prior = stats(&[("uia", tier(60, 39, 0))]);
    let after = apply(
        Some(prior),
        &[outcome("uia", failed("no_observed_delta"))],
        &cfg,
    );
    let uia = tier_of(&after, "uia");
    ensure(counts(&uia) == (30, 20), format!("uia {uia:?}"))
}

fn row_23_outcome_order_does_not_change_counts() -> Check {
    let cfg = table_cfg();
    let a = [outcome("uia", OutcomeKind::Verified)];
    let b = [outcome("uia", failed("no_observed_delta"))];
    let ab = apply(Some(apply(None, &a, &cfg)), &b, &cfg);
    let ba = apply(Some(apply(None, &b, &cfg)), &a, &cfg);
    let key = |row: &ControlRouteStats| {
        let t = tier_of(row, "uia");
        (t.ok, t.fail, t.refused, t.unverified)
    };
    ensure(
        key(&ab) == key(&ba),
        format!("a,b {:?} vs b,a {:?}", key(&ab), key(&ba)),
    )?;
    ensure(
        key(&ab) == (1, 1, 0, 0),
        format!("contrast: both outcomes not counted {:?}", key(&ab)),
    )
}

// --------------------------------------------------------- decode_row rows

fn row_13_corrupt_bytes_rejected_then_fresh_row() -> Check {
    let cfg = table_cfg();
    let corrupt_rows: [&[u8]; 4] = [
        b"\x00\xffnot json{",
        b"",
        b"{\"record_version\":1}",
        b"[1,2,3]",
    ];
    for corrupt in corrupt_rows {
        ensure(
            decode_row(corrupt).is_err(),
            format!("accepted {corrupt:?}"),
        )?;
        let p = plan(&CLICK_ORDER, decode_row(corrupt).ok().as_ref(), &cfg);
        ensure(
            p.order == CLICK_ORDER && !p.used,
            format!("{corrupt:?}: order {:?}", p.order),
        )?;
    }
    let fresh = apply(
        decode_row(b"\x00\xff").ok(),
        &[outcome("uia", OutcomeKind::Verified)],
        &cfg,
    );
    ensure(
        fresh.first_ts_ns == NOW + 1
            && fresh.tiers.len() == 1
            && counts(&tier_of(&fresh, "uia")) == (1, 0),
        format!("next write is not a fresh row: {fresh:?}"),
    )?;
    let bytes = encode_row(&fresh).map_err(|error| format!("encode: {error}"))?;
    let back = decode_row(&bytes).map_err(|error| format!("decode of a fresh row: {error}"))?;
    ensure(back == fresh, "round trip changed the row")
}

fn row_14_unknown_tier_is_dropped() -> Check {
    let cfg = table_cfg();
    let raw = serde_json::json!({
        "record_version": 1,
        "ts_ns": NOW,
        "first_ts_ns": NOW,
        "sig": {
            "exe": "notepad.exe",
            "window_class": "Notepad",
            "role": "button",
            "id_kind": "automation_id",
            "id": "SaveButton"
        },
        "exe_stamp": { "size": STAMP.size, "mtime_ns": STAMP.mtime_ns },
        "tiers": {
            "uia": {
                "ok": 0, "fail": 3, "refused": 0, "unverified": 0,
                "fail_streak": 3, "last_ts_ns": NOW, "last_reason": "no_observed_delta"
            },
            "kernel_inject": {
                "ok": 999, "fail": 0, "refused": 0, "unverified": 0,
                "fail_streak": 0, "last_ts_ns": NOW
            }
        }
    });
    let bytes = serde_json::to_vec(&raw).map_err(|error| error.to_string())?;
    let decoded = decode_row(&bytes)
        .map_err(|error| format!("a valid row with one unknown tier was rejected: {error}"))?;
    ensure(
        !decoded.tiers.contains_key("kernel_inject"),
        "unknown tier kept on decode",
    )?;
    ensure(
        decoded.tiers.contains_key("uia"),
        "known tier lost on decode",
    )?;
    let p = plan(&CLICK_ORDER, Some(&decoded), &cfg);
    ensure(
        !p.order.contains(&"kernel_inject"),
        format!("unknown tier in the order {:?}", p.order),
    )?;
    let planted = stats(&[("uia", tier(0, 3, 3)), ("kernel_inject", tier(999, 0, 0))]);
    let rewritten = apply(
        Some(planted),
        &[
            outcome("uia", OutcomeKind::Verified),
            outcome("kernel_inject", OutcomeKind::Verified),
        ],
        &cfg,
    );
    ensure(
        !rewritten.tiers.contains_key("kernel_inject"),
        "unknown tier survived a rewrite",
    )?;
    ensure(
        counts(&tier_of(&rewritten, "uia")) == (1, 3),
        format!(
            "known tier not updated on rewrite: {:?}",
            tier_of(&rewritten, "uia")
        ),
    )
}

// --------------------------------------------- control_signature / key rows

fn row_17_huge_name_and_id_give_bounded_key_without_raw_text() -> Check {
    let name: String = "secret-user-text "
        .repeat(6_000)
        .chars()
        .take(100_000)
        .collect();
    let automation_id = "z".repeat(10_000);
    let identity = ControlIdentity {
        exe: "notepad.exe",
        window_class: "Edit",
        role: "edit",
        automation_id: &automation_id,
        name: &name,
    };
    let sig = control_signature(&identity).ok_or("no signature for a long but real control")?;
    let key = route_memory_key(&sig, "click");
    ensure(
        key.len() <= ROUTE_MEMORY_KEY_MAX_LEN,
        format!("key length {}", key.len()),
    )?;
    ensure(
        !key.contains("secret") && !key.contains("zzzzzzzz"),
        format!("raw text in key {key}"),
    )?;
    let row = apply_outcome(
        None,
        &sig,
        &[outcome("uia", OutcomeKind::Verified)],
        NOW,
        STAMP,
        &table_cfg(),
    );
    let text = encoded_text(&row)?;
    ensure(
        !text.contains("secret") && !text.contains("zzzzzzzz"),
        "raw name or automation id stored in the value",
    )?;
    let short = control_signature(&ControlIdentity {
        automation_id: "",
        name: "Save",
        ..identity
    })
    .ok_or("no signature for a short name")?;
    ensure(
        route_memory_key(&short, "click").len() == key.len(),
        "key length depends on the input length",
    )
}

fn row_18_same_control_same_key_other_exe_other_key() -> Check {
    let first = ControlIdentity {
        exe: "Notepad.exe",
        window_class: "Notepad",
        role: "button",
        automation_id: "SaveButton",
        name: "Save",
    };
    // A second sighting of the same control through another hwnd / runtime id:
    // those parts are not inputs of the identity, only the stable metadata is.
    let again = ControlIdentity {
        exe: "NOTEPAD.EXE",
        ..first
    };
    let a = control_signature(&first).ok_or("no signature")?;
    let b = control_signature(&again).ok_or("no signature for the second sighting")?;
    ensure(
        route_memory_key(&a, "click") == route_memory_key(&b, "click"),
        "same control, different keys",
    )?;
    let other = control_signature(&ControlIdentity {
        exe: "wordpad.exe",
        ..first
    })
    .ok_or("no signature under the other exe")?;
    ensure(
        route_memory_key(&a, "click") != route_memory_key(&other, "click"),
        "same id under another exe shares a key",
    )
}

fn row_19_digit_runs_share_a_signature_and_empty_has_none() -> Check {
    let item = |name: &'static str| ControlIdentity {
        exe: "explorer.exe",
        window_class: "CabinetWClass",
        role: "list item",
        automation_id: "",
        name,
    };
    let a = control_signature(&item("Item 12"));
    let b = control_signature(&item("Item 97"));
    ensure(a.is_some() && a == b, format!("{a:?} vs {b:?}"))?;
    ensure(
        a.as_ref().map(|s| s.id_kind) == Some(IdKind::NameHash),
        format!("id kind {a:?}"),
    )?;
    ensure(
        control_signature(&item("")).is_none(),
        "empty id and name gave a signature",
    )?;
    ensure(
        control_signature(&item("   ")).is_none(),
        "blank name gave a signature",
    )
}

fn row_20_path_automation_id_is_hashed() -> Check {
    let path = r"C:\Users\someone\Documents\report.txt";
    let identity = ControlIdentity {
        exe: "winword.exe",
        window_class: "OpusApp",
        role: "button",
        automation_id: path,
        name: "",
    };
    let sig = control_signature(&identity).ok_or("no signature")?;
    ensure(
        sig.id_kind == IdKind::AutomationIdHash
            && sig.id != path
            && sig.id.len() == 16
            && !sig.id.contains("report"),
        format!("{sig:?}"),
    )?;
    let key = route_memory_key(&sig, "click");
    let row = apply_outcome(
        None,
        &sig,
        &[outcome("uia", OutcomeKind::Verified)],
        NOW,
        STAMP,
        &table_cfg(),
    );
    let text = encoded_text(&row)?;
    ensure(
        !key.contains("report") && !text.contains("report") && !text.contains("Documents"),
        "the path is stored raw",
    )?;
    let safe = control_signature(&ControlIdentity {
        automation_id: "SaveButton",
        ..identity
    })
    .ok_or("no signature for a safe id")?;
    ensure(
        safe.id_kind == IdKind::AutomationId && safe.id == "SaveButton",
        format!("a safe automation id was not stored as is: {safe:?}"),
    )
}

fn row_25_hostile_exe_stays_inside_the_prefix() -> Check {
    for exe in [r"..\X/y.EXE", "Ünïcödé.exe", "...", "a/../../b.exe"] {
        let identity = ControlIdentity {
            exe,
            window_class: "W",
            role: "button",
            automation_id: "Ok",
            name: "",
        };
        let sig = control_signature(&identity).ok_or_else(|| format!("{exe}: no signature"))?;
        let key = route_memory_key(&sig, "click");
        let segments: Vec<&str> = key.split('/').collect();
        ensure(
            key.starts_with(ROUTE_MEMORY_PREFIX) && segments.len() == 5,
            format!("{exe}: key {key}"),
        )?;
        let exe_segment = segments.get(2).copied().unwrap_or_default();
        ensure(
            !exe_segment.is_empty() && !exe_segment.starts_with('.') && !exe_segment.contains('\\'),
            format!("{exe}: exe segment {exe_segment}"),
        )?;
        ensure(
            key.is_ascii() && !key.chars().any(|c| c.is_ascii_uppercase()),
            format!("{exe}: key not lowercase ascii {key}"),
        )?;
        ensure(
            key.starts_with(&exe_key_prefix(exe)),
            format!(
                "{exe}: key {key} outside its app prefix {}",
                exe_key_prefix(exe)
            ),
        )?;
    }
    ensure(
        exe_key_prefix("ü.exe") != exe_key_prefix("ö.exe"),
        "two unicode exes share one app prefix",
    )
}

// --------------------------------------------------------- prune_plan rows

fn row_24_prune_only_the_full_apps_oldest_row() -> Check {
    let cfg = RouteMemoryConfig {
        max_rows_per_app: 3,
        ..table_cfg()
    };
    let notepad = |sig: &str, ts_ns: u64| PruneCandidate {
        key: format!("route-memory/v1/notepad.exe/click/{sig}"),
        ts_ns,
    };
    let rows = vec![
        PruneCandidate {
            key: "route-memory/v1/calc.exe/click/0000000000000001".to_owned(),
            ts_ns: 1,
        },
        notepad("00000000000000a4", 40),
        notepad("00000000000000a1", 10),
        notepad("00000000000000a3", 30),
        notepad("00000000000000a2", 20),
        PruneCandidate {
            key: "route-memory/v1/notepad.exe~1234abcd/click/00000000000000b1".to_owned(),
            ts_ns: 2,
        },
    ];
    let plan_a = prune_plan("notepad.exe", &rows, &cfg);
    ensure(
        plan_a == vec!["route-memory/v1/notepad.exe/click/00000000000000a1".to_owned()],
        format!("app A plan {plan_a:?}"),
    )?;
    let plan_b = prune_plan("calc.exe", &rows, &cfg);
    ensure(
        plan_b.is_empty(),
        format!("app B under its cap was pruned: {plan_b:?}"),
    )
}

// ---------------------------------------------------------------- the table

#[test]
fn choose_tier_order_table() {
    run(rows![
        row_01_uia_no_delta_postmessage_ok_promotes_postmessage,
        row_02_no_row_keeps_static_order,
        row_03_disabled_keeps_static_order,
        row_04_min_evidence_boundary,
        row_05_confidence_and_margin_boundaries,
        row_06_fail_streak_at_limit_blocks_promotion,
        row_07_earlier_tier_without_failures_is_not_overtaken,
        row_15_foreground_absent_from_static_order_is_never_added,
        row_16_order_is_a_permutation_and_foreground_never_moves_earlier,
    ]);
}

#[test]
fn stats_freshness_table() {
    run(rows![row_21_stale_binary_and_future_rows_are_ignored]);
}

#[test]
fn apply_outcome_table() {
    run(rows![
        row_08_verified_counts_ok_and_resets_streak,
        row_09_no_observed_delta_counts_fail_and_reason,
        row_10_refused_or_deferred_changes_no_ok_fail,
        row_11_delivered_unverified_changes_no_ok_fail,
        row_12_two_attempts_count_on_their_own_tiers,
        row_22_count_cap_halves_and_keeps_ratio,
        row_23_outcome_order_does_not_change_counts,
    ]);
}

#[test]
fn decode_row_table() {
    run(rows![
        row_13_corrupt_bytes_rejected_then_fresh_row,
        row_14_unknown_tier_is_dropped,
    ]);
}

#[test]
fn signature_and_key_table() {
    run(rows![
        row_17_huge_name_and_id_give_bounded_key_without_raw_text,
        row_18_same_control_same_key_other_exe_other_key,
        row_19_digit_runs_share_a_signature_and_empty_has_none,
        row_20_path_automation_id_is_hashed,
        row_25_hostile_exe_stays_inside_the_prefix,
    ]);
}

#[test]
fn prune_plan_table() {
    run(rows![row_24_prune_only_the_full_apps_oldest_row]);
}
