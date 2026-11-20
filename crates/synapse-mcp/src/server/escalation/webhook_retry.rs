//! Off-machine webhook ladder step: a failed channel is retried with backoff
//! before the ladder moves to the next channel (#307, upstream e9abdee3).
//!
//! The decision is pure and table-tested in `webhook_retry_table_307.rs`.

/// Attempts per channel before a failing channel is given up.
pub(super) const WEBHOOK_RETRY_MAX_ATTEMPTS_PER_CHANNEL: u32 = 3;
/// First retry delay; it doubles per failed attempt, never past the ack window.
pub(super) const WEBHOOK_RETRY_BASE_BACKOFF_MS: u64 = 30_000;

/// What the ladder does after one delivery attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WebhookStep {
    /// 1-based number of the attempt just made on this channel.
    pub attempt_number: u32,
    /// Channel index the ladder is on after this attempt.
    pub ladder_index: u32,
    /// Failed attempts recorded on the channel the ladder is now on.
    pub rung_failed_attempts: u32,
    pub next_escalate_at_unix_ms: Option<u64>,
    /// Set when the same channel is retried.
    pub retry_backoff_ms: Option<u64>,
    /// Set when a failing channel used its last attempt and the ladder moved on.
    pub retry_exhausted: bool,
}

/// A failed attempt with attempts left keeps the ladder on the same channel
/// and retries after the backoff. A delivered attempt, or the last failed one,
/// moves to the next channel after the ack window, or stops the ladder when no
/// channel is left. Counters and the clock saturate instead of overflowing.
pub(super) fn next_webhook_step(
    delivered: bool,
    ladder_index: u32,
    rung_failed_attempts: u32,
    channel_count: usize,
    window_ms: u64,
    now_unix_ms: u64,
) -> WebhookStep {
    let attempt_number = rung_failed_attempts.saturating_add(1);
    if !delivered && attempt_number < WEBHOOK_RETRY_MAX_ATTEMPTS_PER_CHANNEL {
        let backoff = retry_backoff_ms(attempt_number, window_ms);
        return WebhookStep {
            attempt_number,
            ladder_index,
            rung_failed_attempts: attempt_number,
            next_escalate_at_unix_ms: Some(now_unix_ms.saturating_add(backoff)),
            retry_backoff_ms: Some(backoff),
            retry_exhausted: false,
        };
    }
    let next = ladder_index.saturating_add(1);
    let channel_left = usize::try_from(next).is_ok_and(|next| next < channel_count);
    WebhookStep {
        attempt_number,
        ladder_index: next,
        rung_failed_attempts: 0,
        next_escalate_at_unix_ms: channel_left.then(|| now_unix_ms.saturating_add(window_ms)),
        retry_backoff_ms: None,
        retry_exhausted: !delivered,
    }
}

/// 30 s after the first failure, doubling per attempt, never past the window.
fn retry_backoff_ms(attempt_number: u32, window_ms: u64) -> u64 {
    let doublings = attempt_number.saturating_sub(1).min(4);
    WEBHOOK_RETRY_BASE_BACKOFF_MS
        .saturating_mul(1_u64 << doublings)
        .min(window_ms)
}

#[cfg(test)]
#[path = "webhook_retry_table_307.rs"]
mod table_307;
