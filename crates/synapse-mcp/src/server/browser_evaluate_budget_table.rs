//! #2331 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row: the time limit of `browser evaluate`.

use super::EvaluateLimit;

#[test]
fn evaluate_budget_table_2331() {
    // (row, timeout_ms given by the caller, expected: Ok(limit) or Err = refused naming timeout_ms)
    let rows: [(&str, Option<u64>, Result<u64, ()>); 10] = [
        ("E1_empty_no_timeout_gives_the_default", None, Ok(5_000)),
        ("E2_normal_longer_limit_is_kept", Some(7_000), Ok(7_000)),
        (
            "E2_neighbour_explicit_default_is_kept",
            Some(5_000),
            Ok(5_000),
        ),
        ("E3_boundary_lowest_accepted", Some(50), Ok(50)),
        ("E3_boundary_highest_accepted", Some(120_000), Ok(120_000)),
        ("E4_boundary_below_lowest_refused", Some(49), Err(())),
        ("E4_boundary_above_highest_refused", Some(120_001), Err(())),
        ("E4_empty_zero_refused", Some(0), Err(())),
        ("E4_hostile_u64_max_refused", Some(u64::MAX), Err(())),
        ("E4_hostile_one_ms_refused", Some(1), Err(())),
    ];
    let mut failures = Vec::new();
    for (row, timeout_ms, expected) in rows {
        let got = EvaluateLimit::from_request(timeout_ms).map(EvaluateLimit::as_millis);
        let pass = match (&got, expected) {
            (Ok(limit), Ok(want)) => *limit == want,
            (Err(message), Err(())) => message.contains("timeout_ms"),
            _ => false,
        };
        if !pass {
            failures.push(format!("row {row}: got {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2331 budget rows failed:\n{}",
        failures.join("\n")
    );
}
