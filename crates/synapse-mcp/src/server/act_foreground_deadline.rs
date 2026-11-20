//! Deadline for the action inside an `act operation=foreground` escalation
//! (#53). The escalation holds the daemon-wide escalation lock and its
//! session fence from claim to unwind, so an action that never returns would
//! pin both: session teardown would wait forever and operator force-release
//! would be refused. Bounding the action guarantees the unwind (restore
//! profile, release lease, drop fence) always runs.

use std::{future::Future, time::Duration};

use synapse_core::error_codes;

use super::{ErrorData, mcp_error};

/// Extra time beyond the lease TTL before the action is abandoned.
pub(crate) const ACT_FOREGROUND_DEADLINE_SLACK_MS: u64 = 5_000;

/// Hard time limit for the escalated action: the lease TTL plus slack.
pub(crate) fn act_foreground_deadline(ttl_ms: u64) -> Duration {
    Duration::from_millis(ttl_ms.saturating_add(ACT_FOREGROUND_DEADLINE_SLACK_MS))
}

/// Runs `action`, abandoning it when `deadline` elapses so the caller can
/// unwind. A timed-out action is dropped (cancelled at its next await point).
pub(crate) async fn run_within_deadline<T>(
    deadline: Duration,
    action: impl Future<Output = Result<T, ErrorData>>,
) -> Result<T, ErrorData> {
    match tokio::time::timeout(deadline, action).await {
        Ok(result) => result,
        Err(_elapsed) => Err(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!(
                "act operation=foreground action exceeded its {} ms deadline (lease ttl + slack); the escalation was unwound",
                deadline.as_millis()
            ),
        )),
    }
}

#[cfg(test)]
#[path = "tests/act_foreground_deadline_53.rs"]
mod tests;
