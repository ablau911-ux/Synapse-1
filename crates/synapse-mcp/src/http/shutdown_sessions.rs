//! #169: shutdown ordering for HTTP sessions.
//!
//! On restart the daemon drains the rmcp session map (to release client
//! sockets) and only afterwards runs the shutdown input cleanup. Two ordering
//! bugs lived there: the cleanup read the already-drained map (so it saw no
//! sessions), and the background stale-session sweeper kept running through the
//! ~2 s drain grace, treating the drained sessions as dead and tearing them
//! down (killing launched apps, closing tabs, deleting persisted state).
//!
//! [`ShutdownSessionGate`] fixes the order: stop and join the sweeper first,
//! then snapshot the live session ids, and hand that snapshot to the cleanup.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Ids of the sessions currently registered in the rmcp session map.
pub(super) async fn active_http_session_ids(
    session_manager: &LocalSessionManager,
) -> BTreeSet<String> {
    session_manager
        .sessions
        .read()
        .await
        .keys()
        .map(|session_id| session_id.as_ref().to_owned())
        .collect()
}

#[derive(Clone)]
pub(super) struct ShutdownSessionGate {
    inner: Arc<GateInner>,
}

struct GateInner {
    sweeper_stop: CancellationToken,
    sweeper_task: Mutex<Option<JoinHandle<()>>>,
    captured: Mutex<BTreeSet<String>>,
}

impl ShutdownSessionGate {
    /// The sweeper stops when either the daemon cancels or [`Self::begin`] runs.
    pub(super) fn new(shutdown_cancel: &CancellationToken) -> Self {
        Self {
            inner: Arc::new(GateInner {
                sweeper_stop: shutdown_cancel.child_token(),
                sweeper_task: Mutex::new(None),
                captured: Mutex::new(BTreeSet::new()),
            }),
        }
    }

    /// Token the stale-session sweeper must select on.
    pub(super) fn sweeper_stop_token(&self) -> CancellationToken {
        self.inner.sweeper_stop.clone()
    }

    pub(super) fn register_sweeper(&self, task: JoinHandle<()>) {
        if let Ok(mut slot) = self.inner.sweeper_task.lock() {
            *slot = Some(task);
        }
    }

    /// Stops and joins the sweeper, then records the live session ids.
    /// Idempotent: later calls keep earlier captures (the map is empty after
    /// the first drain).
    pub(super) async fn begin(&self, session_manager: &LocalSessionManager) -> BTreeSet<String> {
        self.inner.sweeper_stop.cancel();
        let task = self
            .inner
            .sweeper_task
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(task) = task
            && let Err(error) = task.await
        {
            tracing::warn!(
                code = "MCP_HTTP_SESSION_SWEEPER_JOIN_FAILED",
                detail = %error,
                "stale-session sweeper did not stop cleanly before shutdown snapshot"
            );
        }
        let live = active_http_session_ids(session_manager).await;
        match self.inner.captured.lock() {
            Ok(mut captured) => {
                captured.extend(live);
                captured.clone()
            }
            Err(_poisoned) => live,
        }
    }

    /// Sessions the shutdown cleanup must release: everything captured by
    /// [`Self::begin`] plus anything still registered.
    pub(super) async fn sessions_for_cleanup(
        &self,
        session_manager: &LocalSessionManager,
    ) -> BTreeSet<String> {
        let mut sessions = active_http_session_ids(session_manager).await;
        if let Ok(captured) = self.inner.captured.lock() {
            sessions.extend(captured.iter().cloned());
        }
        sessions
    }
}

#[cfg(test)]
#[path = "tests/shutdown_session_gate_169.rs"]
mod shutdown_session_gate_169;
