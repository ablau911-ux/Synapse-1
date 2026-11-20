//! #1699 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the table in the swarm report
//! for Gitea issue #1699 (upstream #1588/#1604): a shell job's timeout is
//! judged from how long the process really lived, not from when the monitor
//! happened to look; a matching remote exit beats a local wrapper timeout.

use super::{
    LocalShellOutcome, RemoteExitMarkerVerdict, ShellChildObservation, ShellExitVerdict,
    classify_shell_child_exit, remote_exit_marker_verdict,
};

const fn seen(
    exited: Option<Option<i32>>,
    timer_fired: bool,
    lifetime_ms: Option<u64>,
    elapsed_ms: u64,
) -> ShellChildObservation {
    ShellChildObservation {
        exited,
        timer_fired,
        lifetime_ms,
        elapsed_ms,
    }
}

const fn verdict(exit_code: Option<i32>, timed_out: bool) -> ShellExitVerdict {
    ShellExitVerdict {
        exit_code,
        timed_out,
    }
}

#[test]
fn shell_child_exit_table_1699() {
    // (row, budget ms, observation, expected verdict)
    let rows: [(&str, u64, ShellChildObservation, ShellExitVerdict); 10] = [
        (
            "1699a_normal_fast_child_late_observer_is_ok",
            1_000,
            seen(Some(Some(0)), false, Some(10), 1_500),
            verdict(Some(0), false),
        ),
        (
            "1699a_normal_fast_child_deadline_delivered_after_exit_keeps_exit",
            1_000,
            seen(Some(Some(0)), true, Some(10), 1_500),
            verdict(Some(0), false),
        ),
        (
            "1699a_normal_nonzero_exit_within_budget_kept",
            1_000,
            seen(Some(Some(7)), false, Some(50), 60),
            verdict(Some(7), false),
        ),
        (
            "1699b_boundary_child_outran_budget_before_observer_times_out",
            40,
            seen(Some(Some(0)), false, Some(150), 400),
            verdict(Some(0), true),
        ),
        (
            "1699c_boundary_lifetime_one_ms_below_budget_is_ok",
            1_000,
            seen(Some(Some(0)), false, Some(999), 999),
            verdict(Some(0), false),
        ),
        (
            "1699c_boundary_lifetime_equal_to_budget_times_out",
            1_000,
            seen(Some(Some(0)), false, Some(1_000), 1_000),
            verdict(Some(0), true),
        ),
        (
            "1699d_hostile_lifetime_unknown_falls_back_to_elapsed_under_budget",
            1_000,
            seen(Some(Some(0)), false, None, 500),
            verdict(Some(0), false),
        ),
        (
            "1699d_hostile_lifetime_unknown_falls_back_to_elapsed_over_budget",
            1_000,
            seen(Some(Some(3)), false, None, 1_200),
            verdict(Some(3), true),
        ),
        (
            "1699e_neighbour_still_running_at_deadline_is_terminated_timeout",
            1_000,
            seen(None, true, None, 1_000),
            verdict(None, true),
        ),
        (
            "1699e_neighbour_exited_without_code_within_budget_is_not_timeout",
            1_000,
            seen(Some(None), false, Some(20), 30),
            verdict(None, false),
        ),
    ];
    let mut failures = Vec::new();
    for (row, budget_ms, observation, expected) in rows {
        let got = classify_shell_child_exit(budget_ms, observation);
        if got != expected {
            failures.push(format!("row {row}: {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#1699 shell exit rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn remote_exit_marker_table_1699() {
    use LocalShellOutcome as L;
    use RemoteExitMarkerVerdict as V;
    // (row, local outcome, marker pid/pgid match the job, remote exit code, expected)
    let rows: [(&str, L, bool, i32, V); 8] = [
        (
            "1699r_remote_exit_0_overrides_local_wrapper_timeout",
            L::TimedOut,
            true,
            0,
            V::Apply {
                overrides_local_timeout: true,
            },
        ),
        (
            "1699r_remote_nonzero_exit_overrides_local_wrapper_timeout",
            L::TimedOut,
            true,
            3,
            V::Apply {
                overrides_local_timeout: true,
            },
        ),
        (
            "1699r_hostile_foreign_marker_ignored_even_on_timeout",
            L::TimedOut,
            false,
            0,
            V::Ignore,
        ),
        (
            "1699r_cancel_wins_over_late_marker",
            L::CancelRequested,
            true,
            0,
            V::Ignore,
        ),
        (
            "1699r_neighbour_stale_wrapper_remote_exit_0_applies",
            L::Other,
            true,
            0,
            V::Apply {
                overrides_local_timeout: false,
            },
        ),
        (
            "1699r_neighbour_nonzero_without_timeout_left_to_ssh_exit_code",
            L::Other,
            true,
            2,
            V::Ignore,
        ),
        (
            "1699r_neighbour_already_ok_job_untouched",
            L::ExitedOk,
            true,
            0,
            V::Ignore,
        ),
        (
            "1699r_hostile_foreign_marker_ignored_on_running_job",
            L::Other,
            false,
            0,
            V::Ignore,
        ),
    ];
    let mut failures = Vec::new();
    for (row, local, matches, code, expected) in rows {
        let got = remote_exit_marker_verdict(local, matches, code);
        if got != expected {
            failures.push(format!("row {row}: {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#1699 remote exit marker rows failed:\n{}",
        failures.join("\n")
    );
}
