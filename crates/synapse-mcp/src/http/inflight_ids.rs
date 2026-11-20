//! #20: per-session registry of JSON-RPC request ids that are still in flight.
//!
//! rmcp's local session worker routes responses by request id. A second POST
//! on the same session that reuses an id still in flight is never answered
//! (the client hangs until its own timeout). This registry lets the session
//! middleware reject that duplicate immediately with an explicit error.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt as _;

type InflightKey = (String, String);

#[derive(Clone, Default)]
pub(super) struct InflightRequestIds {
    keys: Arc<Mutex<HashSet<InflightKey>>>,
    duplicate_rejections: Arc<AtomicU64>,
}

/// Holds one `(session, request id)` claim; released when dropped.
pub(super) struct InflightClaim {
    keys: Arc<Mutex<HashSet<InflightKey>>>,
    key: InflightKey,
}

pub(super) enum ClaimOutcome {
    Held(InflightClaim),
    Unguarded,
    Duplicate,
}

impl Drop for InflightClaim {
    fn drop(&mut self) {
        if let Ok(mut keys) = self.keys.lock() {
            keys.remove(&self.key);
        }
    }
}

impl InflightRequestIds {
    /// Whether any request of `session_id` is still being answered (#214).
    /// A poisoned lock answers `true`, so a sweep never closes on doubt.
    pub(super) fn has_session(&self, session_id: &str) -> bool {
        self.keys.lock().map_or(true, |keys| {
            keys.iter().any(|(session, _)| session == session_id)
        })
    }

    pub(super) fn inflight_count(&self) -> usize {
        self.keys.lock().map_or(0, |keys| keys.len())
    }

    pub(super) fn inflight_count_for_session(&self, session_id: &str) -> usize {
        self.keys.lock().map_or(0, |keys| {
            keys.iter()
                .filter(|(session, _)| session == session_id)
                .count()
        })
    }

    pub(super) fn duplicate_rejections_total(&self) -> u64 {
        self.duplicate_rejections.load(Ordering::Relaxed)
    }

    /// Claims the id for this session. A poisoned lock admits the request
    /// unguarded rather than wedging every later request.
    pub(super) fn claim(&self, session_id: &str, request_id: &str) -> ClaimOutcome {
        let key = (session_id.to_owned(), request_id.to_owned());
        let Ok(mut keys) = self.keys.lock() else {
            return ClaimOutcome::Unguarded;
        };
        if !keys.insert(key.clone()) {
            self.duplicate_rejections.fetch_add(1, Ordering::Relaxed);
            return ClaimOutcome::Duplicate;
        }
        ClaimOutcome::Held(InflightClaim {
            keys: Arc::clone(&self.keys),
            key,
        })
    }
}

/// Keeps `claim` alive until the response body (possibly a long SSE stream)
/// has been fully sent or dropped.
pub(super) fn hold_until_body_done(response: Response, claim: InflightClaim) -> Response {
    let (parts, body) = response.into_parts();
    let stream = body.into_data_stream().map(move |chunk| {
        let _ = &claim;
        chunk
    });
    Response::from_parts(parts, Body::from_stream(stream))
}

/// JSON-RPC error for a duplicate in-flight id; `id_json` is the raw id.
pub(super) fn duplicate_inflight_response(session_id: &str, id_json: &str) -> Response {
    tracing::warn!(
        code = "MCP_HTTP_DUPLICATE_INFLIGHT_REQUEST_ID",
        session_id,
        request_id = id_json,
        "HTTP MCP request rejected: request id already in flight on this session"
    );
    let id = serde_json::from_str::<serde_json::Value>(id_json).unwrap_or(serde_json::Value::Null);
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32600,
            "message": "request id is already in flight on this MCP session; use a unique id per concurrent request",
            "data": {
                "reason": "duplicate_inflight_request_id",
                "session_id": session_id,
                "source_of_truth": "http_session_middleware",
            },
        },
    });
    (
        StatusCode::CONFLICT,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body.to_string(),
    )
        .into_response()
}

/// Raw JSON text of the request id for a single (non-batch) JSON-RPC request.
pub(super) fn jsonrpc_request_id(value: &serde_json::Value) -> Option<String> {
    value.get("method")?;
    let id = value.get("id")?;
    (!id.is_null()).then(|| id.to_string())
}
