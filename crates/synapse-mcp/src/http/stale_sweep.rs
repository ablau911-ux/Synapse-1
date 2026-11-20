//! Stale-session sweep (#210).
//!
//! The daemon used to run lease-expiry release, orphan-lease reconciliation,
//! idle-seat reclaim and every stale-session teardown in one sequential loop.
//! A single teardown stuck on a hung CDP endpoint therefore stopped expired
//! input leases from releasing held keys and buttons for the whole daemon.
//!
//! Two independent periodic loops replace it:
//!
//! * lease maintenance (expiry release + orphan reconciliation): short,
//!   never waits on a teardown;
//! * stale teardown: each teardown runs concurrently (bounded) under its own
//!   timeout, so one hung session cannot hold up the others. A timed-out
//!   teardown stays pinned as `cleanup_failed` and the 30 s retry takes over.

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use futures_util::StreamExt as _;
use rmcp::transport::streamable_http_server::session::{
    SessionManager as _, local::LocalSessionManager,
};
use tokio::{task::JoinHandle, time};
use tokio_util::sync::CancellationToken;

use super::inflight_ids::InflightRequestIds;
use super::shutdown_sessions::active_http_session_ids;
use crate::server::session_lifecycle::SessionLifecycleState;

const SWEEP_INTERVAL: Duration = Duration::from_millis(250);
/// Upper bound for one stale-session teardown (or idle-seat reclaim pass).
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(120);
/// Stale teardowns running at once; the rest of a large backlog queues.
const MAX_CONCURRENT_TEARDOWNS: usize = 8;

/// Starts the sweep; the returned handle finishes once `shutdown_cancel` fires.
pub(super) fn spawn(
    session_manager: Arc<LocalSessionManager>,
    session_lifecycle: SessionLifecycleState,
    inflight_ids: InflightRequestIds,
    shutdown_cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::join!(
            run_lease_maintenance(&session_lifecycle, &shutdown_cancel),
            run_stale_teardown(
                &session_lifecycle,
                &session_manager,
                &inflight_ids,
                &shutdown_cancel
            ),
        );
    })
}

/// Runs lease maintenance every [`SWEEP_INTERVAL`] until `cancel` fires. Each
/// tick runs to completion so a lease release is never cut in half.
async fn run_lease_maintenance(
    session_lifecycle: &SessionLifecycleState,
    cancel: &CancellationToken,
) {
    let mut interval = time::interval(SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = interval.tick() => {
                lease_maintenance_once(session_lifecycle).await;
            }
        }
    }
    tracing::debug!(
        code = "MCP_HTTP_SESSION_INPUT_STALE_CLEANUP_STOPPED",
        label = "lease_maintenance",
        "stopping stale HTTP session cleanup loop"
    );
}

/// Runs stale teardown every [`SWEEP_INTERVAL`] until `cancel` fires. Ticks are
/// dropped mid-flight on cancel (teardown is cancellation-safe).
async fn run_stale_teardown(
    session_lifecycle: &SessionLifecycleState,
    session_manager: &LocalSessionManager,
    inflight_ids: &InflightRequestIds,
    cancel: &CancellationToken,
) {
    let mut interval = time::interval(SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = interval.tick() => {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = stale_teardown_once(session_lifecycle, session_manager, inflight_ids) => {}
                }
            }
        }
    }
    tracing::debug!(
        code = "MCP_HTTP_SESSION_INPUT_STALE_CLEANUP_STOPPED",
        label = "stale_teardown",
        "stopping stale HTTP session cleanup loop"
    );
}

async fn lease_maintenance_once(session_lifecycle: &SessionLifecycleState) {
    session_lifecycle.cleanup_expired_lease_inputs_once().await;
    session_lifecycle.reconcile_orphaned_lease_rows_once().await;
}

async fn stale_teardown_once(
    session_lifecycle: &SessionLifecycleState,
    session_manager: &LocalSessionManager,
    inflight_ids: &InflightRequestIds,
) {
    close_terminated_http_sessions(session_lifecycle, session_manager, inflight_ids).await;
    let active_session_count = active_http_session_ids(session_manager).await.len();
    let (retry_sessions, dead_owner_retries, protected_sessions) =
        session_lifecycle.failed_cleanup_retry_candidates().await;
    let mut protected_active = active_http_session_ids(session_manager).await;
    protected_active.extend(protected_sessions);
    let mut stale_sessions = session_lifecycle.stale_session_candidates(&protected_active);
    for session_id in crate::server::session_lifecycle::retry_teardown_ids(
        &retry_sessions,
        &dead_owner_retries,
        &protected_active,
    ) {
        stale_sessions.insert(session_id, "http_stale");
    }
    reclaim_idle_seats_bounded(session_lifecycle).await;
    // Bounded-concurrency teardown without stream-combinator closures: closures
    // over the borrowed lifecycle make the spawned sweep future fail the
    // higher-ranked `Send` check ("FnOnce is not general enough").
    let mut failed_cleanup_sessions = BTreeSet::new();
    let mut queue = stale_sessions.into_iter();
    let mut running = futures_util::stream::FuturesUnordered::new();
    loop {
        while running.len() < MAX_CONCURRENT_TEARDOWNS {
            let Some((session_id, reason)) = queue.next() else {
                break;
            };
            running.push(teardown_one(
                session_lifecycle,
                session_id,
                reason,
                active_session_count,
            ));
        }
        match running.next().await {
            Some(Some(failed_session)) => {
                failed_cleanup_sessions.insert(failed_session);
            }
            Some(None) => {}
            None => break,
        }
    }
    session_lifecycle
        .prune_expired_registry_entries(session_manager, &failed_cleanup_sessions)
        .await;
}

/// #214: a tool-initiated teardown (`agent_kill`, `adopt`) only marks the
/// session terminated; it has no handle on the rmcp session. Close those
/// sessions here so their worker is released, they stop counting as active, and
/// failed-cleanup retries are no longer suppressed by a phantom keep-alive.
/// Returns the ids that were closed.
async fn close_terminated_http_sessions(
    session_lifecycle: &SessionLifecycleState,
    session_manager: &LocalSessionManager,
    inflight_ids: &InflightRequestIds,
) -> Vec<String> {
    let active = active_http_session_ids(session_manager).await;
    let mut closed = Vec::new();
    let closable = closable_terminated_sessions(
        session_lifecycle.terminated_among(&active),
        |session_id| session_lifecycle.session_teardown_in_progress(session_id),
        |session_id| inflight_ids.has_session(session_id),
    );
    for session_id in closable {
        match session_manager
            .close_session(&Arc::<str>::from(session_id.as_str()))
            .await
        {
            Ok(()) => {
                tracing::info!(
                    code = "MCP_HTTP_SESSION_TERMINATED_CLOSED",
                    session_id = %session_id,
                    "readback=rmcp_session_map after=terminated_session_closed"
                );
                closed.push(session_id);
            }
            Err(error) => tracing::warn!(
                code = "MCP_HTTP_SESSION_TERMINATED_CLOSE_FAILED",
                session_id = %session_id,
                detail = %error,
                "terminated session could not be closed in the rmcp manager; retrying next sweep"
            ),
        }
    }
    closed
}

/// Terminated sessions the sweep may close now (#214): not while the session's
/// own teardown is still running (e.g. a self `session end`), and not while a
/// request of that session is still being answered, so its last response is
/// delivered before the rmcp session goes away.
fn closable_terminated_sessions(
    terminated: Vec<String>,
    teardown_in_progress: impl Fn(&str) -> bool,
    has_inflight_request: impl Fn(&str) -> bool,
) -> Vec<String> {
    terminated
        .into_iter()
        .filter(|session_id| !teardown_in_progress(session_id) && !has_inflight_request(session_id))
        .collect()
}

#[cfg(test)]
#[path = "tests/stale_sweep_close_214.rs"]
mod stale_sweep_close_214_tests;

async fn reclaim_idle_seats_bounded(session_lifecycle: &SessionLifecycleState) {
    if time::timeout(
        TEARDOWN_TIMEOUT,
        session_lifecycle.reclaim_idle_seat_owners_once(),
    )
    .await
    .is_err()
    {
        tracing::error!(
            code = "MCP_HTTP_SESSION_IDLE_SEAT_RECLAIM_TIMEOUT",
            timeout_ms = TEARDOWN_TIMEOUT.as_millis(),
            "idle seat-owner reclaim did not finish in time; retrying next sweep"
        );
    }
}

/// Tears one stale session down under [`TEARDOWN_TIMEOUT`]. Returns the session
/// id when the cleanup failed or timed out, so retention keeps its metadata.
async fn teardown_one(
    session_lifecycle: &SessionLifecycleState,
    session_id: String,
    reason: &'static str,
    active_session_count: usize,
) -> Option<String> {
    match time::timeout(
        TEARDOWN_TIMEOUT,
        session_lifecycle.teardown_session(&session_id, reason),
    )
    .await
    {
        Ok(Ok(report)) => {
            tracing::info!(
                code = "MCP_HTTP_SESSION_STALE_LIFECYCLE_CLEANUP",
                session_id = %session_id,
                reason,
                active_session_count,
                report = ?report,
                "readback=session_lifecycle edge=http_session_gone after_cleanup"
            );
            (report.failure_count > 0).then_some(session_id)
        }
        Ok(Err(error)) => {
            tracing::error!(
                code = synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                session_id = %session_id,
                reason,
                active_session_count,
                detail = %error.message,
                data = ?error.data,
                "HTTP MCP stale-session lifecycle cleanup failed"
            );
            Some(session_id)
        }
        Err(_elapsed) => {
            tracing::error!(
                code = "MCP_HTTP_SESSION_STALE_TEARDOWN_TIMEOUT",
                session_id = %session_id,
                reason,
                active_session_count,
                timeout_ms = TEARDOWN_TIMEOUT.as_millis(),
                "HTTP MCP stale-session teardown timed out; left pinned for the retry path"
            );
            Some(session_id)
        }
    }
}

#[cfg(test)]
#[path = "tests/terminated_session_close_214.rs"]
mod terminated_session_close_214_tests;
