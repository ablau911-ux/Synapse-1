//! #307 webhook-rung frozen behaviour table (manager-owned, work method v3).
//! Codex and Opus never edit this file. One case per L row of the webhook
//! retry table in Gitea issue #307.

use super::{WebhookStep, next_webhook_step};

const NOW: u64 = 1_000_000;
const WINDOW: u64 = 60_000;
const CHANNELS: usize = 2;

/// (row, delivered, ladder index, failed attempts on rung, channel count,
/// ack window, now, expected step)
type Case = (&'static str, bool, u32, u32, usize, u64, u64, WebhookStep);

const fn step(
    attempt_number: u32,
    ladder_index: u32,
    rung_failed_attempts: u32,
    next_escalate_at_unix_ms: Option<u64>,
    retry_backoff_ms: Option<u64>,
    retry_exhausted: bool,
) -> WebhookStep {
    WebhookStep {
        attempt_number,
        ladder_index,
        rung_failed_attempts,
        next_escalate_at_unix_ms,
        retry_backoff_ms,
        retry_exhausted,
    }
}

#[test]
fn webhook_retry_step_table_307() {
    let rows: [Case; 11] = [
        (
            "307w_a_delivered_first_channel_moves_on_after_the_ack_window",
            true,
            0,
            0,
            CHANNELS,
            WINDOW,
            NOW,
            step(1, 1, 0, Some(NOW + WINDOW), None, false),
        ),
        (
            "307w_b_failed_first_attempt_retries_the_same_channel_after_30s",
            false,
            0,
            0,
            CHANNELS,
            WINDOW,
            NOW,
            step(1, 0, 1, Some(NOW + 30_000), Some(30_000), false),
        ),
        (
            "307w_c_failed_second_attempt_doubles_the_backoff",
            false,
            0,
            1,
            CHANNELS,
            WINDOW,
            NOW,
            step(2, 0, 2, Some(NOW + 60_000), Some(60_000), false),
        ),
        (
            "307w_d_boundary_failed_third_attempt_gives_up_and_moves_on",
            false,
            0,
            2,
            CHANNELS,
            WINDOW,
            NOW,
            step(3, 1, 0, Some(NOW + WINDOW), None, true),
        ),
        (
            "307w_e_failed_third_attempt_on_the_last_channel_stops_the_ladder",
            false,
            1,
            2,
            CHANNELS,
            WINDOW,
            NOW,
            step(3, 2, 0, None, None, true),
        ),
        (
            "307w_f_delivered_on_the_last_channel_stops_the_ladder",
            true,
            1,
            0,
            CHANNELS,
            WINDOW,
            NOW,
            step(1, 2, 0, None, None, false),
        ),
        (
            "307w_g_delivered_after_failures_resets_the_count_and_moves_on",
            true,
            0,
            2,
            CHANNELS,
            WINDOW,
            NOW,
            step(3, 1, 0, Some(NOW + WINDOW), None, false),
        ),
        (
            "307w_h_hostile_short_ack_window_caps_the_backoff",
            false,
            0,
            0,
            CHANNELS,
            2_000,
            NOW,
            step(1, 0, 1, Some(NOW + 2_000), Some(2_000), false),
        ),
        (
            "307w_h_hostile_zero_window_retries_on_the_next_tick",
            false,
            0,
            0,
            CHANNELS,
            0,
            NOW,
            step(1, 0, 1, Some(NOW), Some(0), false),
        ),
        (
            "307w_i_hostile_counters_at_max_saturate_and_stop",
            false,
            u32::MAX,
            u32::MAX,
            CHANNELS,
            WINDOW,
            NOW,
            step(u32::MAX, u32::MAX, 0, None, None, true),
        ),
        (
            "307w_j_hostile_clock_near_max_saturates",
            false,
            0,
            0,
            CHANNELS,
            WINDOW,
            u64::MAX - 10,
            step(1, 0, 1, Some(u64::MAX), Some(30_000), false),
        ),
    ];
    let mut failures = Vec::new();
    for (row, delivered, ladder, failed, channels, window, now, expected) in rows {
        let got = next_webhook_step(delivered, ladder, failed, channels, window, now);
        if got != expected {
            failures.push(format!("row {row}: step {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#307 webhook retry rows failed:\n{}",
        failures.join("\n")
    );
}
