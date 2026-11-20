//! Retention of finished approval and escalation queue rows (#307).
//!
//! The decision is pure and table-tested in `terminal_retention_table_307.rs`.
//! The approval queue (`m3::approvals`) and the escalation queue
//! (`server::escalation`) each summarize their own rows and delete the keys
//! this plan returns; each keeps its own status rules.

/// How long a finished row is kept and how many finished rows may stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RetentionPolicy {
    pub max_age_ms: u64,
    pub max_terminal_rows: usize,
}

/// Shipped policy for both queues: finished rows live 7 days, and at most
/// 5,000 of them stay, well under the 20,000-row scan limit.
pub(crate) const TERMINAL_RETENTION: RetentionPolicy = RetentionPolicy {
    max_age_ms: 7 * 24 * 60 * 60 * 1_000,
    max_terminal_rows: 5_000,
};

/// What retention needs to know about one queue row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueueRowSummary {
    pub key: Vec<u8>,
    /// The row reached a final status that never changes again.
    pub terminal: bool,
    /// A follow-up for this row has not run yet (for example the on-PC toast
    /// of a closed escalation was never removed).
    pub cleanup_pending: bool,
    pub updated_at_unix_ms: u64,
}

/// Keys of finished rows to delete, in key order. Only terminal rows without a
/// pending follow-up are candidates; open rows are never returned. A candidate
/// goes when it is at least `max_age_ms` old (a future timestamp counts as
/// age 0), and the oldest surviving candidates (ties by key) go until at most
/// `max_terminal_rows` remain.
pub(crate) fn plan_terminal_prune(
    rows: &[QueueRowSummary],
    now_unix_ms: u64,
    policy: RetentionPolicy,
) -> Vec<Vec<u8>> {
    let mut finished: Vec<&QueueRowSummary> = rows
        .iter()
        .filter(|row| row.terminal && !row.cleanup_pending)
        .collect();
    finished.sort_by(|a, b| {
        a.updated_at_unix_ms
            .cmp(&b.updated_at_unix_ms)
            .then_with(|| a.key.cmp(&b.key))
    });
    let expired = finished
        .iter()
        .take_while(|row| now_unix_ms.saturating_sub(row.updated_at_unix_ms) >= policy.max_age_ms)
        .count();
    let over_cap = (finished.len() - expired).saturating_sub(policy.max_terminal_rows);
    let mut keys: Vec<Vec<u8>> = finished
        .iter()
        .take(expired + over_cap)
        .map(|row| row.key.clone())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
#[path = "terminal_retention_table_307.rs"]
mod table_307;
