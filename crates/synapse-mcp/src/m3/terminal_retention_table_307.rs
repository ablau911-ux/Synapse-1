//! #307 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the table in Gitea issue #307.

use super::{QueueRowSummary, RetentionPolicy, TERMINAL_RETENTION, plan_terminal_prune};

const NOW: u64 = 10_000;
const POLICY: RetentionPolicy = RetentionPolicy {
    max_age_ms: 1_000,
    max_terminal_rows: 3,
};

fn row(key: &str, terminal: bool, cleanup_pending: bool, updated_at: u64) -> QueueRowSummary {
    QueueRowSummary {
        key: key.as_bytes().to_vec(),
        terminal,
        cleanup_pending,
        updated_at_unix_ms: updated_at,
    }
}

fn done(key: &str, updated_at: u64) -> QueueRowSummary {
    row(key, true, false, updated_at)
}

/// (row name, queue rows, now, policy, expected deleted keys in key order)
type Case = (
    &'static str,
    Vec<QueueRowSummary>,
    u64,
    RetentionPolicy,
    Vec<Vec<u8>>,
);

fn keys(names: &[&str]) -> Vec<Vec<u8>> {
    names.iter().map(|name| name.as_bytes().to_vec()).collect()
}

#[test]
fn terminal_prune_plan_table_307() {
    // (row, queue rows, now, policy, expected deleted keys in key order)
    let rows: Vec<Case> = vec![
        (
            "307a_normal_old_terminal_row_is_pruned",
            vec![done("item/old", NOW - 5_000)],
            NOW,
            POLICY,
            keys(&["item/old"]),
        ),
        (
            "307b_empty_queue_prunes_nothing",
            Vec::new(),
            NOW,
            POLICY,
            Vec::new(),
        ),
        (
            "307c_boundary_one_ms_below_cutoff_is_kept",
            vec![done("item/young", NOW - 999)],
            NOW,
            POLICY,
            Vec::new(),
        ),
        (
            "307c_boundary_exactly_at_cutoff_is_pruned",
            vec![done("item/edge", NOW - 1_000)],
            NOW,
            POLICY,
            keys(&["item/edge"]),
        ),
        (
            "307d_hostile_future_timestamp_is_kept",
            vec![done("item/future", NOW + 5_000)],
            NOW,
            POLICY,
            Vec::new(),
        ),
        (
            "307d_hostile_extreme_timestamps_do_not_overflow",
            vec![done("item/max", u64::MAX), done("item/zero", 0)],
            0,
            POLICY,
            Vec::new(),
        ),
        (
            "307e_neighbour_open_rows_are_never_pruned_even_old_and_many",
            (0..6)
                .map(|index| row(&format!("item/open-{index}"), false, false, 0))
                .collect(),
            NOW,
            POLICY,
            Vec::new(),
        ),
        (
            "307f_linked_cleanup_pending_keeps_old_terminal_row",
            vec![row("item/toast-not-removed", true, true, 0)],
            NOW,
            POLICY,
            Vec::new(),
        ),
        (
            "307f_cleanup_pending_rows_do_not_count_toward_the_cap",
            vec![
                row("item/p1", true, true, NOW),
                row("item/p2", true, true, NOW),
                done("item/r1", NOW - 3),
                done("item/r2", NOW - 2),
                done("item/r3", NOW - 1),
            ],
            NOW,
            POLICY,
            Vec::new(),
        ),
        (
            "307g_cap_prunes_the_oldest_recent_terminal_rows",
            vec![
                done("item/e", NOW - 1),
                done("item/a", NOW - 5),
                done("item/c", NOW - 3),
                done("item/b", NOW - 4),
                done("item/d", NOW - 2),
                row("item/open", false, false, NOW - 900),
            ],
            NOW,
            POLICY,
            keys(&["item/a", "item/b"]),
        ),
        (
            "307g_cap_tie_on_timestamp_is_broken_by_key",
            vec![
                done("item/k4", NOW),
                done("item/k2", NOW),
                done("item/k1", NOW),
                done("item/k3", NOW),
            ],
            NOW,
            POLICY,
            keys(&["item/k1"]),
        ),
        (
            "307g_age_and_cap_together_delete_each_key_once",
            vec![
                done("item/old1", 0),
                done("item/old2", 1),
                done("item/n1", NOW - 4),
                done("item/n2", NOW - 3),
                done("item/n3", NOW - 2),
                done("item/n4", NOW - 1),
            ],
            NOW,
            POLICY,
            keys(&["item/n1", "item/old1", "item/old2"]),
        ),
    ];
    let mut failures = Vec::new();
    for (name, queue, now, policy, expected) in rows {
        let got = plan_terminal_prune(&queue, now, policy);
        if got != expected {
            failures.push(format!(
                "row {name}: deleted {:?}, expected {:?}",
                printable(&got),
                printable(&expected)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "#307 retention rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn terminal_prune_recovers_an_oversized_queue_table_307() {
    // 307h cap recovery: 25,000 recent finished rows (over the old 20,000-row
    // scan limit) plus one live row, under the shipped policy.
    let now = 100 * TERMINAL_RETENTION.max_age_ms;
    let mut queue: Vec<QueueRowSummary> = (0..25_000_u64)
        .map(|index| done(&format!("item/done-{index:05}"), now - 25_000 + index))
        .collect();
    queue.push(row("item/live", false, false, 0));
    let got = plan_terminal_prune(&queue, now, TERMINAL_RETENTION);
    let mut failures = Vec::new();
    if TERMINAL_RETENTION.max_terminal_rows != 5_000
        || TERMINAL_RETENTION.max_age_ms != 7 * 24 * 60 * 60 * 1_000
    {
        failures.push(format!(
            "row 307h_shipped_policy_is_7_days_and_5000_rows: got {TERMINAL_RETENTION:?}"
        ));
    }
    if got.len() != 20_000 {
        failures.push(format!(
            "row 307h_cap_recovery_prunes_down_to_the_cap: deleted {} rows, expected 20000",
            got.len()
        ));
    }
    if got.iter().any(|key| key.as_slice() == b"item/live") {
        failures.push("row 307h_cap_recovery_keeps_the_live_row: live row deleted".to_owned());
    }
    let newest_kept = format!("item/done-{:05}", 24_999).into_bytes();
    let oldest_deleted = format!("item/done-{:05}", 0).into_bytes();
    if got.contains(&newest_kept) || !got.contains(&oldest_deleted) {
        failures.push("row 307h_cap_recovery_deletes_oldest_first: wrong rows selected".to_owned());
    }
    assert!(
        failures.is_empty(),
        "#307 cap-recovery rows failed:\n{}",
        failures.join("\n")
    );
}

fn printable(keys: &[Vec<u8>]) -> Vec<String> {
    keys.iter()
        .map(|key| String::from_utf8_lossy(key).into_owned())
        .collect()
}
