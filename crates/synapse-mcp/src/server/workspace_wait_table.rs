//! #2329 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the `workspace wait` table.

use super::WorkspaceWait;

#[test]
fn workspace_wait_budget_table_2329() {
    // (row, timeout_ms, poll_interval_ms, expected: None = accepted, Some(field) = refused naming the field)
    let rows: [(&str, u64, u64, Option<&str>); 9] = [
        ("W1_normal_defaults_accepted", 5_000, 50, None),
        ("W2_boundary_lowest_accepted", 1, 1, None),
        ("W2_boundary_highest_accepted", 60_000, 500, None),
        ("W3_empty_timeout_zero_refused", 0, 50, Some("timeout_ms")),
        (
            "W3_boundary_timeout_above_max_refused",
            60_001,
            50,
            Some("timeout_ms"),
        ),
        (
            "W3_hostile_timeout_u64_max_refused",
            u64::MAX,
            50,
            Some("timeout_ms"),
        ),
        (
            "W3_empty_poll_zero_refused",
            5_000,
            0,
            Some("poll_interval_ms"),
        ),
        (
            "W3_boundary_poll_above_max_refused",
            5_000,
            501,
            Some("poll_interval_ms"),
        ),
        (
            "W3_hostile_poll_u64_max_refused",
            5_000,
            u64::MAX,
            Some("poll_interval_ms"),
        ),
    ];
    let mut failures = Vec::new();
    for (row, timeout_ms, poll_interval_ms, expected) in rows {
        let got = WorkspaceWait::new(None, String::new(), timeout_ms, poll_interval_ms).map(drop);
        let pass = match (&got, expected) {
            (Ok(()), None) => true,
            (Err(message), Some(field)) => message.contains(field),
            _ => false,
        };
        if !pass {
            failures.push(format!("row {row}: got {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2329 budget rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn workspace_wait_sleep_table_2329() {
    // (row, now_ms, deadline_ms, poll_interval_ms, expected sleep)
    let rows: [(&str, u64, u64, u64, Option<u64>); 6] = [
        ("W4_normal_full_interval", 0, 1_000, 50, Some(50)),
        (
            "W4_boundary_interval_cut_to_time_left",
            980,
            1_000,
            50,
            Some(20),
        ),
        ("W4_boundary_one_ms_left", 999, 1_000, 50, Some(1)),
        ("W4_boundary_deadline_reached_stops", 1_000, 1_000, 50, None),
        (
            "W4_hostile_now_after_deadline_stops_without_underflow",
            1_500,
            1_000,
            50,
            None,
        ),
        ("W4_hostile_zero_interval_sleeps_one_ms", 0, 10, 0, Some(1)),
    ];
    let mut failures = Vec::new();
    for (row, now_ms, deadline_ms, poll_interval_ms, expected) in rows {
        let wait = WorkspaceWait {
            run_id: None,
            key: String::new(),
            timeout_ms: deadline_ms,
            poll_interval_ms,
        };
        let got = wait.next_sleep_ms(now_ms);
        if got != expected {
            failures.push(format!("row {row}: sleep {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2329 sleep rows failed:\n{}",
        failures.join("\n")
    );
}
