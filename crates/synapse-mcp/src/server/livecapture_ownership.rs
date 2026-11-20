//! Server glue for livecapture session ownership (#112): which sessions are
//! still open, the stop/retarget gate, and the owner readback.

use super::{ErrorData, SynapseService, mcp_error, session_registry::unix_time_ms_now};
use crate::m1::M1State;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Instant;
use synapse_core::error_codes;

/// Sessions that were open at `taken_at`. The instant lets the mutation gate
/// tell an owner that is genuinely gone from one that registered and claimed
/// the capture after this snapshot was taken (#112).
pub(super) struct OpenSessions {
    ids: HashSet<String>,
    taken_at: Instant,
}

impl SynapseService {
    /// Ids of sessions that have not terminated. Taken before the M1 state lock
    /// so the registry and M1 locks are never nested. A session missing from
    /// the registry counts as terminated.
    pub(super) fn open_session_ids(&self) -> Result<OpenSessions, ErrorData> {
        // Captured before the registry read: any session absent from the read
        // registered, and so claimed, strictly after this instant.
        let taken_at = Instant::now();
        let registry = self.session_registry_ref().lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session registry lock poisoned while resolving livecapture owner liveness",
            )
        })?;
        let ids = registry
            .reads(unix_time_ms_now())
            .into_iter()
            .filter(|read| read.lifecycle != "closed")
            .map(|read| read.session_id)
            .collect();
        Ok(OpenSessions { ids, taken_at })
    }

    /// Owner of the active capture, for `status`: `null` when unowned.
    pub(super) fn livecapture_owner_readback(&self) -> Result<Value, ErrorData> {
        let state = self.m1_state()?;
        let active = active_capture_id(&state);
        Ok(state
            .capture_owner
            .owner_of(active.as_deref())
            .map_or(Value::Null, |owner| {
                json!({
                    "owner_session_id": owner.session_id,
                    "capture_id": owner.capture_id,
                })
            }))
    }
}

pub(super) fn active_capture_id(state: &M1State) -> Option<String> {
    state
        .capture_controller
        .active()
        .filter(|handle| handle.is_running())
        .map(|handle| handle.capture_id().to_owned())
}

/// Whether a failed `start` must tear its capture down (#110, #122). A worker
/// that already ended is always cleaned up, whoever started or adopted it, so
/// adopters can never leave a dead owner reported as running. A healthy worker
/// is stopped only by the sole creator; an adopted one stays for its adopter.
pub(super) const fn failed_start_should_stop(
    created_here: bool,
    has_joiners: bool,
    worker_running: bool,
) -> bool {
    !worker_running || (created_here && !has_joiners)
}

/// Refuses `operation` (`stop` / `start`-retarget) when another live session
/// owns the active capture; the error names the owner.
pub(super) fn ensure_capture_mutation_allowed(
    state: &M1State,
    operation: &str,
    caller_session_id: Option<&str>,
    open_sessions: &OpenSessions,
) -> Result<(), ErrorData> {
    let active = active_capture_id(state);
    state
        .capture_owner
        .check_mutation_with_snapshot(
            active.as_deref(),
            caller_session_id,
            open_sessions.taken_at,
            |session| open_sessions.ids.contains(session),
        )
        .map_err(|conflict| {
            mcp_error(
                error_codes::CAPTURE_OWNED_BY_OTHER_SESSION,
                conflict.message(operation),
            )
        })
}

#[cfg(test)]
#[path = "tests/livecapture_ownership_service_112.rs"]
mod livecapture_ownership_service_112_tests;
