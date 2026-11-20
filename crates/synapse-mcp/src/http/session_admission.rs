//! HTTP MCP session admission cap (#13).
//!
//! rmcp's default `LocalSessionManager` spawns a worker and a map entry for
//! every `initialize` with no upper bound, and sequential initializes never hit
//! the connection pool. This is a resource bound, not an access restriction:
//! any local caller is still admitted while capacity remains.
//!
//! A session counts against the cap from the moment its `initialize` is
//! reserved until it leaves the manager map (DELETE, idle expiry, teardown), so
//! closing or expiring a session releases capacity with no extra bookkeeping.

use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use anyhow::{Context, bail};
use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

pub(super) const MAX_SESSIONS_ENV: &str = "SYNAPSE_HTTP_MAX_SESSIONS";
const DEFAULT_MAX_SESSIONS: usize = 1024;
const HARD_MAX_SESSIONS: usize = 4096;
pub(super) const SESSION_CAPACITY_CODE: &str = "HTTP_SESSION_CAPACITY";

#[derive(Clone)]
pub(super) struct SessionAdmission {
    max: usize,
    /// Initializes admitted but not yet visible in the session manager map.
    pending: Arc<AtomicUsize>,
    rejected_total: Arc<AtomicU64>,
    /// Serializes admission decisions so the check and the reservation are one step.
    gate: Arc<tokio::sync::Mutex<()>>,
}

/// Releases the pending reservation once the initialize response head has been
/// produced (by then the session is in the manager map, or the attempt failed).
pub(super) struct AdmissionGuard {
    pending: Arc<AtomicUsize>,
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) struct AdmissionDenied {
    pub active: usize,
    pub pending: usize,
    pub max: usize,
    pub rejected_total: u64,
}

impl Default for SessionAdmission {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_SESSIONS)
    }
}

impl SessionAdmission {
    pub(super) fn new(max: usize) -> Self {
        Self {
            max,
            pending: Arc::default(),
            rejected_total: Arc::default(),
            gate: Arc::default(),
        }
    }

    pub(super) fn from_env() -> anyhow::Result<Self> {
        // An invalid override must not stop the daemon (same policy as the
        // #56/#60 interval settings): warn and keep the default cap.
        let raw = match std::env::var(MAX_SESSIONS_ENV) {
            Ok(raw) => raw,
            Err(std::env::VarError::NotPresent) => return Ok(Self::default()),
            Err(error) => {
                tracing::warn!(
                    code = "HTTP_MAX_SESSIONS_INVALID",
                    env = MAX_SESSIONS_ENV,
                    %error,
                    "unreadable session cap override; using default"
                );
                return Ok(Self::default());
            }
        };
        match Self::parse_max(&raw) {
            Ok(max) => Ok(Self::new(max)),
            Err(error) => {
                tracing::warn!(
                    code = "HTTP_MAX_SESSIONS_INVALID",
                    env = MAX_SESSIONS_ENV,
                    value = %raw,
                    error = %format!("{error:#}"),
                    "invalid session cap override; using default"
                );
                Ok(Self::default())
            }
        }
    }

    pub(super) fn max_sessions(&self) -> usize {
        self.max
    }

    pub(super) fn rejected_total(&self) -> u64 {
        self.rejected_total.load(Ordering::SeqCst)
    }

    pub(super) fn parse_max(raw: &str) -> anyhow::Result<usize> {
        let value = raw.trim();
        let max = value
            .parse::<usize>()
            .with_context(|| format!("invalid integer {value:?}"))?;
        if max == 0 {
            bail!("max sessions must be greater than zero");
        }
        if max > HARD_MAX_SESSIONS {
            bail!("max sessions must not exceed {HARD_MAX_SESSIONS}, got {max}");
        }
        Ok(max)
    }

    /// Reserve one session slot, or report the occupancy that refused it.
    ///
    /// `pending` is read before the live count: a reservation released between
    /// the two reads is then counted twice (conservative), never zero times.
    pub(super) async fn admit(
        &self,
        manager: &LocalSessionManager,
    ) -> Result<AdmissionGuard, AdmissionDenied> {
        let _decision = self.gate.lock().await;
        let pending = self.pending.load(Ordering::SeqCst);
        let active = manager.sessions.read().await.len();
        if active.saturating_add(pending) >= self.max {
            let rejected_total = self.rejected_total.fetch_add(1, Ordering::SeqCst) + 1;
            return Err(AdmissionDenied {
                active,
                pending,
                max: self.max,
                rejected_total,
            });
        }
        self.pending.fetch_add(1, Ordering::SeqCst);
        Ok(AdmissionGuard {
            pending: Arc::clone(&self.pending),
        })
    }
}

/// 503 with a machine-readable occupancy readback.
pub(super) fn capacity_exceeded_response(denied: &AdmissionDenied) -> Response {
    tracing::warn!(
        code = SESSION_CAPACITY_CODE,
        active_sessions = denied.active,
        pending_initializes = denied.pending,
        max_sessions = denied.max,
        rejected_total = denied.rejected_total,
        "HTTP MCP initialize refused: session capacity reached"
    );
    let body = serde_json::json!({
        "code": SESSION_CAPACITY_CODE,
        "reason": "session_capacity_reached",
        "active_sessions": denied.active,
        "pending_initializes": denied.pending,
        "max_sessions": denied.max,
        "rejected_total": denied.rejected_total,
        "recovery": "close idle sessions (DELETE /mcp with Mcp-Session-Id) or raise SYNAPSE_HTTP_MAX_SESSIONS, then retry",
        "source_of_truth": "http_session_admission",
    });
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::RETRY_AFTER, "5"),
        ],
        body.to_string(),
    )
        .into_response()
}
