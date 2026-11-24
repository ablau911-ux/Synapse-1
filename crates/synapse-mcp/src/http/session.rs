use std::{
    collections::BTreeSet,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context as TaskContext, Poll},
    time::Duration,
};

use anyhow::{Context, bail};
use axum::{
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rmcp::model::ErrorCode;
use rmcp::transport::streamable_http_server::session::local::{LocalSessionManager, SessionConfig};

const SESSION_IDLE_TIMEOUT_ENV: &str = "SYNAPSE_HTTP_SESSION_IDLE_TIMEOUT_SECS";
const DEFAULT_SESSION_IDLE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
const MAX_MCP_REQUEST_BYTES: usize = 1024 * 1024;
const SESSION_ID_HEADER: &str = "Mcp-Session-Id";

tokio::task_local! {
    static CURRENT_MCP_SESSION_ID: Option<String>;
}

#[derive(Clone)]
pub(super) struct SessionRequestState {
    session_registry: crate::server::session_registry::SharedSessionRegistry,
    terminated_sessions: crate::server::session_lifecycle::SharedTerminatedSessions,
    in_flight_request_ids: SharedInFlightRequestIds,
}

/// JSON-RPC request ids in flight per session: (session id, id as JSON text).
type SharedInFlightRequestIds = Arc<Mutex<BTreeSet<(String, String)>>>;

/// One JSON-RPC request id in flight in one session (#243). rmcp routes a
/// response to its request's stream by the id alone, so a second request with
/// the same id in the same session takes over the first one's stream: one
/// caller gets the other's result and the other stream never ends. The id is
/// held from the request until rmcp ends the request's response stream: rmcp
/// ends it after routing the request's response, on the client's cancellation
/// notification, when the session closes, or at daemon shutdown. A client that
/// stops reading the stream does not end the request in rmcp.
struct InFlightRequestId {
    in_flight: SharedInFlightRequestIds,
    key: (String, String),
    request_id: serde_json::Value,
}

impl InFlightRequestId {
    /// Claims `message`'s id in `session_id`, or refuses the request with
    /// -32600 while another request with that id is in flight there. A message
    /// that is not a request (a notification, a response) claims nothing.
    fn claim(
        in_flight: &SharedInFlightRequestIds,
        session_id: &str,
        message: &serde_json::Value,
    ) -> Result<Option<Self>, Response> {
        let Some(id) = message.get("id") else {
            return Ok(None);
        };
        if !message
            .get("method")
            .is_some_and(serde_json::Value::is_string)
        {
            return Ok(None);
        }
        let key = (session_id.to_owned(), id.to_string());
        let claimed = in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone());
        if !claimed {
            return Err(invalid_jsonrpc_request(
                id,
                "a request with this id is still in flight in this session; use a new id",
            ));
        }
        Ok(Some(Self {
            in_flight: Arc::clone(in_flight),
            key,
            request_id: id.clone(),
        }))
    }

    /// Keeps the id claimed until rmcp ends `response`'s body, also when the
    /// client stops reading it first.
    fn hold_until_request_ends(self, response: Response) -> Response {
        let request_id = self.request_id.clone();
        let session_id = self.key.0.clone();
        InFlightBody::wrap(response, request_id, Some(session_id), Some(self))
    }
}

impl Drop for InFlightRequestId {
    fn drop(&mut self) {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// A request's response body that keeps the request's in-flight id claimed
/// until rmcp ends the body.
struct InFlightBody {
    data: axum::body::BodyDataStream,
    /// `None` once `Drop` handed the claim to the task that reads the rest of
    /// rmcp's stream.
    in_flight: Option<InFlightRequestId>,
    /// The request's session; `None` for an initialize that has none yet.
    session_id: Option<String>,
    /// `None` when the body is not an SSE stream.
    reply_watch: Option<SseReplyWatch>,
    finished: bool,
}

impl InFlightBody {
    /// Wraps `response`'s body for the request `request_id`. An SSE body that
    /// ends without that request's response gets a JSON-RPC error as its last
    /// event (#258). `in_flight` is the request's id claim, `None` for an
    /// initialize, which opens its session and so cannot share an id there.
    fn wrap(
        response: Response,
        request_id: serde_json::Value,
        session_id: Option<String>,
        in_flight: Option<InFlightRequestId>,
    ) -> Response {
        let is_event_stream = response.status() == StatusCode::OK
            && response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/event-stream"));
        let reply_watch = is_event_stream.then(|| SseReplyWatch::new(request_id));
        let (parts, body) = response.into_parts();
        let body = Self {
            data: body.into_data_stream(),
            in_flight,
            session_id,
            reply_watch,
            finished: false,
        };
        Response::from_parts(parts, Body::from_stream(body))
    }
}

impl futures_util::Stream for InFlightBody {
    type Item = Result<axum::body::Bytes, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        let polled = futures_util::Stream::poll_next(Pin::new(&mut this.data), cx);
        match &polled {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Some(watch) = this.reply_watch.as_mut() {
                    watch.observe(chunk);
                }
            }
            Poll::Ready(None) => {
                this.finished = true;
                let session_id = this.session_id.as_deref();
                if let Some(event) = this
                    .reply_watch
                    .take()
                    .and_then(|watch| watch.missing_reply_event(session_id))
                {
                    return Poll::Ready(Some(Ok(event)));
                }
            }
            Poll::Ready(Some(Err(_))) | Poll::Pending => {}
        }
        polled
    }
}

impl Drop for InFlightBody {
    /// The client stopped reading before rmcp ended the stream (#243). rmcp
    /// 1.7.0 does not cancel the request then: its service task runs the
    /// handler to completion and sends the response (`service.rs`,
    /// `serve_inner`), and its session worker routes that response by the
    /// JSON-RPC id to whichever stream registered the id last (`session/
    /// local.rs`, `resolve_outbound_channel`). Releasing the id now would let
    /// a new request with the same id take the old response, so the rest of
    /// rmcp's stream is read to its end and the id released then.
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let Some(in_flight) = self.in_flight.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut data = std::mem::replace(&mut self.data, Body::empty().into_data_stream());
        runtime.spawn(async move {
            while let Some(Ok(_chunk)) = futures_util::StreamExt::next(&mut data).await {}
            drop(in_flight);
        });
    }
}

/// Watches one request's SSE response stream for that request's response
/// (#258). rmcp ends the stream without any message when the session closes
/// (DELETE, idle expiry), the daemon stops, or the client cancels the request,
/// so the caller got HTTP 200 with no result and no error and could not tell
/// whether the call ran.
struct SseReplyWatch {
    request_id: serde_json::Value,
    /// The current SSE line, up to its `\n`.
    line: Vec<u8>,
    answered: bool,
}

/// The members of an SSE `data:` message that tell whether it answers a request.
#[derive(serde::Deserialize)]
struct SseReply {
    id: Option<serde_json::Value>,
    result: Option<serde::de::IgnoredAny>,
    error: Option<serde::de::IgnoredAny>,
}

impl SseReplyWatch {
    const fn new(request_id: serde_json::Value) -> Self {
        Self {
            request_id,
            line: Vec::new(),
            answered: false,
        }
    }

    fn observe(&mut self, mut chunk: &[u8]) {
        if self.answered {
            return;
        }
        while let Some(end) = chunk.iter().position(|byte| *byte == b'\n') {
            self.line.extend_from_slice(&chunk[..end]);
            self.answered = Self::answers(&self.line, &self.request_id);
            self.line.clear();
            if self.answered {
                return;
            }
            chunk = &chunk[end + 1..];
        }
        self.line.extend_from_slice(chunk);
    }

    /// Whether `line` is an SSE `data:` line holding the response or error for
    /// `request_id`.
    fn answers(line: &[u8], request_id: &serde_json::Value) -> bool {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(data) = line.strip_prefix(b"data:") else {
            return false;
        };
        let data = data.strip_prefix(b" ").unwrap_or(data);
        serde_json::from_slice::<SseReply>(data).is_ok_and(|reply| {
            reply.id.as_ref() == Some(request_id)
                && (reply.result.is_some() || reply.error.is_some())
        })
    }

    /// The SSE event that ends an unanswered stream: a JSON-RPC error for the
    /// request, `None` when the response was sent.
    fn missing_reply_event(self, session_id: Option<&str>) -> Option<axum::body::Bytes> {
        if self.answered {
            return None;
        }
        tracing::warn!(
            code = "MCP_HTTP_RESPONSE_STREAM_ENDED_WITHOUT_REPLY",
            session_id = session_id.unwrap_or_default(),
            request_id = %self.request_id,
            "HTTP MCP response stream ended before the request's response; sent a JSON-RPC error instead"
        );
        let detail = "the response stream ended before this request's response was sent (the session closed, the daemon stopped, or the request was cancelled); the request may or may not have run";
        let body = jsonrpc_error_body(
            &self.request_id,
            ErrorCode::INTERNAL_ERROR,
            format!("internal error: {detail}"),
            Some(serde_json::json!({
                "reason": "response_stream_ended_without_reply",
                "outcome": "unknown",
                "session_id": session_id,
                "source_of_truth": "http_session_middleware",
            })),
        );
        Some(axum::body::Bytes::from(format!("data: {body}\n\n")))
    }
}

#[derive(Clone)]
pub(super) struct SessionCleanupState {
    session_manager: Arc<LocalSessionManager>,
    lifecycle: crate::server::session_lifecycle::SessionLifecycleState,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SessionFailure {
    /// No `Mcp-Session-Id` header at all — the caller never initialized a
    /// session on this daemon.
    Missing,
    /// A session id was presented but this daemon's session manager does not
    /// know it: the daemon idle-expired it, or the daemon restarted and lost
    /// all in-memory sessions. The daemon is alive (it answered), so this is a
    /// session-level fact, not a daemon crash.
    UnknownOrExpired,
    /// The session was explicitly torn down by the session lifecycle layer
    /// (stale-eviction, `session_end`, or `agent_kill`). Distinct from
    /// `UnknownOrExpired` because the teardown is intentional and the bound
    /// target row was persisted, so re-binding (adopting) the same browser tab
    /// is the right recovery rather than re-creating it.
    Terminated,
}

impl SessionFailure {
    /// Stable, machine-matchable reason token for the returned diagnostic.
    fn reason(self) -> &'static str {
        match self {
            Self::Missing => "session_header_missing",
            Self::UnknownOrExpired => "session_unknown_or_expired",
            Self::Terminated => "session_terminated",
        }
    }

    /// Which of the three failure classes #1360 part 3 asks callers to
    /// distinguish this is. All HTTP-layer session failures are
    /// `session_level` (the daemon answered with a 404 diagnostic, so the
    /// daemon itself did not crash and the chrome bridge was never reached);
    /// target-invalidation and recoverable-bridge failures surface instead as
    /// tool-level error codes (e.g. `CAPTURE_TARGET_INVALID`) on a *valid*
    /// session.
    fn failure_class(self) -> &'static str {
        "session_level"
    }

    /// The recovery action a caller should take. `Missing` never had a session
    /// to rebind; the other two had a target binding (persisted across
    /// teardown), so the caller should re-create a session then re-bind the
    /// same target instead of re-creating the tab.
    fn recovery(self) -> &'static str {
        match self {
            Self::Missing => "initialize_session",
            Self::UnknownOrExpired | Self::Terminated => "recreate_session_then_rebind_target",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Missing => {
                "no Mcp-Session-Id header was presented; initialize an MCP session before calling tools"
            }
            Self::UnknownOrExpired => {
                "the daemon is alive but does not recognize this session id (idle-expired or daemon restarted); create a new session and re-bind your target"
            }
            Self::Terminated => {
                "the session lifecycle terminated this session (stale-eviction / session_end / agent_kill); create a new session and re-bind the same target (its binding was persisted)"
            }
        }
    }
}

impl SessionCleanupState {
    pub(super) fn request_state(
        session_registry: crate::server::session_registry::SharedSessionRegistry,
        terminated_sessions: crate::server::session_lifecycle::SharedTerminatedSessions,
    ) -> SessionRequestState {
        SessionRequestState {
            session_registry,
            terminated_sessions,
            in_flight_request_ids: Arc::default(),
        }
    }

    pub(super) fn new(
        session_manager: Arc<LocalSessionManager>,
        lifecycle: crate::server::session_lifecycle::SessionLifecycleState,
    ) -> Self {
        Self {
            session_manager,
            lifecycle,
        }
    }
}

pub(crate) fn current_mcp_session_id() -> Option<String> {
    CURRENT_MCP_SESSION_ID.try_with(Clone::clone).ok().flatten()
}

pub(super) fn load_session_config() -> anyhow::Result<SessionConfig> {
    let mut config = SessionConfig::default();
    let idle_timeout_secs = session_idle_timeout_secs()?;
    config.keep_alive = Some(Duration::from_secs(idle_timeout_secs));
    tracing::info!(
        code = "MCP_HTTP_SESSION_CONFIGURED",
        idle_timeout_s = idle_timeout_secs,
        "HTTP MCP session lifecycle configured"
    );
    Ok(config)
}

pub(super) async fn require_mcp_session(
    State(state): State<SessionRequestState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !is_mcp_endpoint(request.uri().path()) {
        return next.run(request).await;
    }
    let session_id = session_id_from_header(&request);
    let (request, initialize_id) = match enforce_session_header(request).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let (request, in_flight) = match session_id.as_deref() {
        Some(session_id) => {
            if let Some(failure) =
                terminated_session_failure(&state.terminated_sessions, session_id)
            {
                if failure == SessionFailure::Terminated && request.method() == Method::DELETE {
                    tracing::info!(
                        code = "MCP_HTTP_SESSION_DELETE_ALREADY_TERMINATED",
                        session_id,
                        "HTTP MCP session DELETE allowed as an idempotent already-terminated cleanup trigger"
                    );
                    let scoped_session_id = Some(session_id.to_owned());
                    return CURRENT_MCP_SESSION_ID
                        .scope(scoped_session_id, next.run(request))
                        .await;
                }
                tracing::warn!(
                    code = synapse_core::error_codes::HTTP_SESSION_INVALID,
                    session_id,
                    reason = failure.reason(),
                    "HTTP MCP session rejected because session lifecycle already terminated it"
                );
                return session_invalid_for(failure, Some(session_id), &serde_json::Value::Null);
            }
            match record_session_request(&state, session_id, request).await {
                Ok(recorded) => recorded,
                Err(response) => return response,
            }
        }
        None => (request, None),
    };
    let diagnostic_session_id = session_id.clone();
    CURRENT_MCP_SESSION_ID
        .scope(session_id, async move {
            let response = next.run(request).await;
            if response.status() == StatusCode::NOT_FOUND {
                return session_invalid_for(
                    SessionFailure::UnknownOrExpired,
                    diagnostic_session_id.as_deref(),
                    &serde_json::Value::Null,
                );
            }
            match (in_flight, initialize_id) {
                (Some(in_flight), _) => in_flight.hold_until_request_ends(response),
                (None, Some(initialize_id)) => {
                    // rmcp names the new session in the initialize response.
                    let new_session_id = response
                        .headers()
                        .get(SESSION_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    InFlightBody::wrap(response, initialize_id, new_session_id, None)
                }
                (None, None) => response,
            }
        })
        .await
}

pub(super) async fn release_held_inputs_on_delete(
    State(state): State<SessionCleanupState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let cleanup_session_id = (request.method() == Method::DELETE
        && is_mcp_endpoint(request.uri().path()))
    .then(|| session_id_from_header(&request))
    .flatten();
    // #277: DELETE of a terminated session is an idempotent 200; DELETE of an
    // unknown or idle-expired one is 404, also after the stale reaper ran.
    let terminated_sessions = state.lifecycle.terminated_sessions();
    let terminated_failure = cleanup_session_id
        .as_deref()
        .and_then(|session_id| terminated_session_failure(terminated_sessions, session_id));
    let already_terminated = terminated_failure == Some(SessionFailure::Terminated);
    if let Some(session_id) = cleanup_session_id.as_deref()
        && !session_is_active(&state.session_manager, session_id).await
    {
        if already_terminated {
            tracing::info!(
                code = "MCP_HTTP_SESSION_DELETE_ALREADY_CLOSED_NOOP",
                session_id,
                "HTTP MCP session DELETE accepted as idempotent no-op for an already-closed terminated session"
            );
            return StatusCode::OK.into_response();
        }
        tracing::warn!(
            code = synapse_core::error_codes::HTTP_SESSION_INVALID,
            session_id,
            reason = ?SessionFailure::UnknownOrExpired,
            "HTTP MCP session delete rejected before held-input cleanup"
        );
        return session_invalid_for(
            SessionFailure::UnknownOrExpired,
            Some(session_id),
            &serde_json::Value::Null,
        );
    }
    // #277: a session the lifecycle already terminated (e.g. by adoption) can
    // still be open in rmcp's session manager. Its DELETE still closes it, but
    // answers like every other DELETE of a terminated session.
    let response = next.run(request).await;
    let Some(session_id) = cleanup_session_id else {
        return response;
    };
    if !response.status().is_success() {
        return response;
    }

    match state
        .lifecycle
        .teardown_session(&session_id, "http_delete")
        .await
    {
        Ok(report) => {
            tracing::info!(
                code = "MCP_HTTP_SESSION_LIFECYCLE_CLEANUP",
                session_id,
                report = ?report,
                "readback=session_lifecycle edge=http_delete after_cleanup"
            );
            if already_terminated {
                StatusCode::OK.into_response()
            } else {
                response
            }
        }
        Err(error) => {
            tracing::error!(
                code = synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                session_id,
                detail = %error.message,
                data = ?error.data,
                "HTTP MCP session lifecycle cleanup failed"
            );
            lifecycle_cleanup_failed(error)
        }
    }
}

async fn session_is_active(session_manager: &LocalSessionManager, session_id: &str) -> bool {
    session_manager
        .sessions
        .read()
        .await
        .contains_key(session_id)
}

/// The failure class of a session the lifecycle tore down, `None` when it did
/// not (#277). The stale reaper's teardown (`HTTP_STALE_REASON`) only reclaims
/// what an idle-expired or pre-restart session left behind, so that session
/// stays `UnknownOrExpired`, the answer it got before the reaper ran. Any other
/// teardown (DELETE, `session_end`, `agent_kill`, adoption, an exited spawned
/// agent) is an intentional `Terminated`.
fn terminated_session_failure(
    terminated_sessions: &crate::server::session_lifecycle::SharedTerminatedSessions,
    session_id: &str,
) -> Option<SessionFailure> {
    let terminated = terminated_sessions.lock().ok()?;
    let reason = terminated.get(session_id)?;
    let failure = if reason == crate::server::session_lifecycle::HTTP_STALE_REASON {
        SessionFailure::UnknownOrExpired
    } else {
        SessionFailure::Terminated
    };
    Some(failure)
}

fn session_idle_timeout_secs() -> anyhow::Result<u64> {
    match std::env::var(SESSION_IDLE_TIMEOUT_ENV) {
        Ok(raw) => parse_idle_timeout(&raw)
            .with_context(|| format!("parse {SESSION_IDLE_TIMEOUT_ENV}={raw:?}")),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_SESSION_IDLE_TIMEOUT_SECS),
        Err(error) => Err(error).with_context(|| format!("read {SESSION_IDLE_TIMEOUT_ENV}")),
    }
}

fn parse_idle_timeout(raw: &str) -> anyhow::Result<u64> {
    let value = raw.trim();
    let seconds = value
        .parse::<u64>()
        .with_context(|| format!("invalid integer {value:?}"))?;
    if seconds == 0 {
        bail!("idle timeout must be greater than zero seconds");
    }
    Ok(seconds)
}

/// Admits a request with a session header, or an initialize POST without one;
/// the second value is the initialize request's id.
async fn enforce_session_header(
    request: Request<Body>,
) -> Result<(Request<Body>, Option<serde_json::Value>), Response> {
    if has_session_header(&request) {
        return Ok((request, None));
    }
    if request.method() == Method::POST {
        allow_initialize_without_session(request).await
    } else if request.method() == Method::GET || request.method() == Method::DELETE {
        Err(session_invalid(
            SessionFailure::Missing,
            &serde_json::Value::Null,
        ))
    } else {
        Ok((request, None))
    }
}

fn has_session_header(request: &Request<Body>) -> bool {
    session_id_from_header(request).is_some()
}

fn session_id_from_header(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get(SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn allow_initialize_without_session(
    request: Request<Body>,
) -> Result<(Request<Body>, Option<serde_json::Value>), Response> {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, MAX_MCP_REQUEST_BYTES)
        .await
        .map_err(|_| payload_too_large())?;
    let message = jsonrpc_message(&bytes)?;
    if !jsonrpc_method_is_initialize(&message) {
        let request_id = message.get("id").unwrap_or(&serde_json::Value::Null);
        return Err(session_invalid(SessionFailure::Missing, request_id));
    }
    let initialize_id = message.get("id").cloned();
    Ok((Request::from_parts(parts, Body::from(bytes)), initialize_id))
}

async fn record_session_request(
    state: &SessionRequestState,
    session_id: &str,
    request: Request<Body>,
) -> Result<(Request<Body>, Option<InFlightRequestId>), Response> {
    if request.method() != Method::POST {
        record_session_heartbeat(&state.session_registry, session_id, None)?;
        return Ok((request, None));
    }

    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, MAX_MCP_REQUEST_BYTES)
        .await
        .map_err(|_| payload_too_large())?;
    let message = jsonrpc_message(&bytes);
    let action = message.as_ref().ok().and_then(jsonrpc_action_label);
    record_session_heartbeat(&state.session_registry, session_id, action)?;
    let in_flight = InFlightRequestId::claim(&state.in_flight_request_ids, session_id, &message?)?;
    Ok((Request::from_parts(parts, Body::from(bytes)), in_flight))
}

/// Checks that a buffered `/mcp` POST body is one JSON-RPC 2.0 message rmcp can
/// route, and answers a JSON-RPC error otherwise (#189): rmcp answers a body it
/// cannot decode with a plain-text 415, which no JSON-RPC client can match.
fn jsonrpc_message(bytes: &[u8]) -> Result<serde_json::Value, Response> {
    // A syntax error anywhere in the body is a parse error (-32700). The shape
    // check below stops at the first wrong shape (an array, a scalar, a
    // repeated member) before serde_json reads the rest of the body, so the
    // whole body is checked for well-formed JSON first (#207, #264).
    serde_json::from_slice::<serde::de::IgnoredAny>(bytes)
        .map_err(|error| jsonrpc_parse_error(&error))?;
    let JsonRpcObject(message) =
        serde_json::from_slice::<JsonRpcObject>(bytes).map_err(|error| {
            if error.classify() == serde_json::error::Category::Data {
                invalid_jsonrpc_request(&serde_json::Value::Null, &error.to_string())
            } else {
                jsonrpc_parse_error(&error)
            }
        })?;
    // A present id that is not a string or an integer (null, object, bool,
    // float, array) fails rmcp's request decode, and rmcp then reads the
    // message as a notification: HTTP 202, never executed, never answered
    // (#207, #264).
    let id = match message.get("id") {
        None => &serde_json::Value::Null,
        Some(id) if id.is_string() || id.as_i64().is_some() => id,
        Some(_) => {
            return Err(invalid_jsonrpc_request(
                &serde_json::Value::Null,
                "id must be a string or an integer (null is not allowed)",
            ));
        }
    };
    if message.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") {
        return Err(invalid_jsonrpc_request(id, "jsonrpc must be \"2.0\""));
    }
    let has_method = message
        .get("method")
        .is_some_and(serde_json::Value::is_string);
    let has_id = message.contains_key("id");
    let is_response = has_id && (message.contains_key("result") || message.contains_key("error"));
    if !has_method && !is_response {
        return Err(invalid_jsonrpc_request(
            id,
            "a message needs a string `method`, or an `id` with `result` or `error`",
        ));
    }
    // rmcp decodes the raw body as its typed `ClientJsonRpcMessage` (rmcp
    // 1.7.0 `expect_json`) and answers a failure with a plain-text 415 (#189).
    // The checks above read a parsed `Value`, which keeps the last of two
    // nested members with one name where the typed decode refuses them, so
    // the same typed decode runs here on the same raw bytes.
    match serde_json::from_slice::<rmcp::model::ClientJsonRpcMessage>(bytes) {
        Err(error) if !has_method && message.contains_key("error") => {
            return Err(invalid_jsonrpc_request(
                id,
                &format!(
                    "`error` must be an object with an integer `code` and a string `message`: {error}"
                ),
            ));
        }
        Err(error) => {
            return Err(invalid_jsonrpc_request(
                id,
                &format!("the message does not decode as an MCP message: {error}"),
            ));
        }
        // rmcp reads a message with an id and a method that does not decode as
        // a request as a notification: HTTP 202, never run, never answered
        // (#264).
        Ok(decoded)
            if has_method
                && has_id
                && !matches!(&decoded, rmcp::model::JsonRpcMessage::Request(_)) =>
        {
            return Err(invalid_jsonrpc_request(
                id,
                "the message does not decode as an MCP request for its `method`; check its `params`",
            ));
        }
        Ok(_) => {}
    }
    Ok(serde_json::Value::Object(message))
}

/// One `/mcp` body: a single JSON object whose top level repeats no member
/// name. serde_json's `Value` keeps the last of two `"id"` members silently,
/// while rmcp's typed request refuses the duplicate and reads the message as a
/// notification, so the request is dropped with a 202 (#264).
struct JsonRpcObject(serde_json::Map<String, serde_json::Value>);

impl<'de> serde::Deserialize<'de> for JsonRpcObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonRpcObjectVisitor)
    }
}

struct JsonRpcObjectVisitor;

impl<'de> serde::de::Visitor<'de> for JsonRpcObjectVisitor {
    type Value = JsonRpcObject;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("one JSON-RPC message object (batches are not supported)")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut members = serde_json::Map::new();
        while let Some(name) = map.next_key::<String>()? {
            if members.contains_key(&name) {
                return Err(serde::de::Error::custom(format_args!(
                    "duplicate member `{name}`"
                )));
            }
            let value = map.next_value()?;
            members.insert(name, value);
        }
        Ok(JsonRpcObject(members))
    }
}

fn jsonrpc_parse_error(error: &serde_json::Error) -> Response {
    jsonrpc_error_response(
        StatusCode::BAD_REQUEST,
        &serde_json::Value::Null,
        ErrorCode::PARSE_ERROR,
        format!("parse error: {error}"),
        None,
    )
}

fn invalid_jsonrpc_request(id: &serde_json::Value, message: &str) -> Response {
    jsonrpc_error_response(
        StatusCode::BAD_REQUEST,
        id,
        ErrorCode::INVALID_REQUEST,
        format!("invalid request: {message}"),
        None,
    )
}

/// A JSON-RPC 2.0 error response the HTTP layer sends for an /mcp request it
/// refuses before rmcp sees it (#189, #266). `id` is `null` when the request
/// id could not be read; `data` carries the refusal's machine-readable fields.
pub(super) fn jsonrpc_error_response(
    status: StatusCode,
    id: &serde_json::Value,
    code: ErrorCode,
    message: String,
    data: Option<serde_json::Value>,
) -> Response {
    let body = jsonrpc_error_body(id, code, message, data);
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// A JSON-RPC 2.0 error message for request `id`.
fn jsonrpc_error_body(
    id: &serde_json::Value,
    code: ErrorCode,
    message: String,
    data: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut error = serde_json::json!({
        "code": code.0,
        "message": message,
    });
    if let Some(data) = data {
        error["data"] = data;
    }
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    })
}

fn record_session_heartbeat(
    session_registry: &crate::server::session_registry::SharedSessionRegistry,
    session_id: &str,
    action: Option<String>,
) -> Result<(), Response> {
    let mut registry = session_registry.lock().map_err(|_error| {
        tracing::error!(
            code = synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            session_id,
            "HTTP MCP session request could not lock cross-session registry"
        );
        session_registry_failed()
    })?;
    registry.record_seen(
        session_id,
        action,
        crate::server::session_registry::unix_time_ms_now(),
    );
    Ok(())
}

fn jsonrpc_action_label(value: &serde_json::Value) -> Option<String> {
    let method = value.get("method")?.as_str()?;
    if method == "tools/call"
        && let Some(name) = value
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(serde_json::Value::as_str)
    {
        return Some(format!("tools/call:{name}"));
    }
    Some(method.to_owned())
}

fn jsonrpc_method_is_initialize(value: &serde_json::Value) -> bool {
    value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|method| method == "initialize")
}

fn is_mcp_endpoint(path: &str) -> bool {
    path == "/mcp" || path.starts_with("/mcp/")
}

const SESSION_RECOVERY_HEADER: &str = "mcp-session-recovery";

fn session_invalid(failure: SessionFailure, request_id: &serde_json::Value) -> Response {
    session_invalid_for(failure, None, request_id)
}

/// Build the 404 session-invalid response with an actionable, machine-matchable
/// diagnostic (#1360 part 3). The body is a JSON-RPC error (#266) whose
/// `error.data` carries the failure class and the recovery the caller should
/// take, and whose `error.data.code` is the literal `HTTP_SESSION_INVALID`
/// string so existing plaintext sniffers still match. Status stays 404 so rmcp
/// transports keep treating it as session-expired. `request_id` is `null` when
/// the body was not read.
fn session_invalid_for(
    failure: SessionFailure,
    session_id: Option<&str>,
    request_id: &serde_json::Value,
) -> Response {
    tracing::warn!(
        code = synapse_core::error_codes::HTTP_SESSION_INVALID,
        reason = failure.reason(),
        failure_class = failure.failure_class(),
        recovery = failure.recovery(),
        session_id = session_id.unwrap_or(""),
        "HTTP MCP session rejected"
    );
    let data = serde_json::json!({
        "code": synapse_core::error_codes::HTTP_SESSION_INVALID,
        "reason": failure.reason(),
        // #1360 part 3: a 404 here means the daemon answered, so it is not a
        // daemon crash and the chrome bridge was never reached. This is a
        // session-level failure; target-invalidation / recoverable-bridge
        // failures surface as tool-level codes on a *valid* session.
        "failure_class": failure.failure_class(),
        "daemon_alive": true,
        "recovery": failure.recovery(),
        "session_id": session_id,
        "detail": failure.detail(),
        "source_of_truth": "http_session_middleware",
    });
    let mut response = jsonrpc_error_response(
        StatusCode::NOT_FOUND,
        request_id,
        ErrorCode::INVALID_REQUEST,
        format!(
            "{}: {}",
            synapse_core::error_codes::HTTP_SESSION_INVALID,
            failure.detail()
        ),
        Some(data),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static(SESSION_RECOVERY_HEADER),
        header::HeaderValue::from_static(failure.recovery()),
    );
    response
}

/// 413 for a body over [`MAX_MCP_REQUEST_BYTES`], as a JSON-RPC error that
/// states the limit (#266). The id is `null`: the body was not read.
fn payload_too_large() -> Response {
    jsonrpc_error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        &serde_json::Value::Null,
        ErrorCode::INVALID_REQUEST,
        format!(
            "invalid request: the body exceeds the {MAX_MCP_REQUEST_BYTES}-byte limit of an /mcp request"
        ),
        None,
    )
}

fn lifecycle_cleanup_failed(error: rmcp::ErrorData) -> Response {
    jsonrpc_error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &serde_json::Value::Null,
        ErrorCode::INTERNAL_ERROR,
        format!(
            "{}: {}",
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            error.message
        ),
        Some(serde_json::json!({
            "code": synapse_core::error_codes::TOOL_INTERNAL_ERROR,
        })),
    )
}

fn session_registry_failed() -> Response {
    jsonrpc_error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &serde_json::Value::Null,
        ErrorCode::INTERNAL_ERROR,
        "SESSION_REGISTRY_UNAVAILABLE".to_owned(),
        Some(serde_json::json!({
            "code": "SESSION_REGISTRY_UNAVAILABLE",
        })),
    )
}
