//! Health reporting for the storage write batcher (#33).

use synapse_storage::{BatcherStatus, Db};

/// Subsystem status reported while the batcher is refusing writes.
pub(super) const DEGRADED_STATUS: &str = "degraded_writes";

/// Reason text when the batcher has a latched commit failure, else `None`.
pub(super) fn degraded_detail(status: &BatcherStatus) -> Option<String> {
    if !status.failed {
        return None;
    }
    Some(format!(
        "storage write batcher failed and is refusing writes: {} \
         (consecutive_failures={}, recoveries={}, next_recovery_probe_in_ms={})",
        status.detail.as_deref().unwrap_or("unknown failure"),
        status.consecutive_failures,
        status.recoveries,
        status.retry_in_ms.unwrap_or(0),
    ))
}

pub(super) fn degraded_detail_for(db: &Db) -> Option<String> {
    degraded_detail(&db.batcher_status())
}

#[cfg(test)]
#[path = "tests/health_batcher_33.rs"]
mod tests;
