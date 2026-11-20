//! Request-shape guards for `POST /mcp` that run in the session middleware,
//! before rmcp sees the request: malformed-JSON rejection (#28) and
//! repeated-`initialize` rejection on a live session (#29).

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

const JSON_MIME: &str = "application/json";
const JSONRPC_PARSE_ERROR: i64 = -32700;
const JSONRPC_INVALID_REQUEST: i64 = -32600;

/// Match the complete media type, ignoring case and optional parameters.
pub(super) fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case(JSON_MIME))
        })
}

/// Preserve rmcp's non-JSON handling, but close its prefix-match loophole.
pub(super) fn normalize_json_content_type(headers: &mut HeaderMap) -> Result<(), Response> {
    if is_json_content_type(headers) {
        // rmcp compares case-sensitively, although media types are insensitive.
        headers.insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static(JSON_MIME),
        );
    } else if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with(JSON_MIME)
        })
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response());
    }
    Ok(())
}

/// True when the JSON-RPC payload is, or (as a batch) contains, `initialize`.
pub(super) fn contains_initialize(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.iter().any(contains_initialize),
        other => other
            .get("method")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|method| method == "initialize"),
    }
}

/// 400 with a JSON-RPC `-32700` parse error for a body that is not valid JSON.
pub(super) fn malformed_json_response(error: &serde_json::Error) -> Response {
    tracing::warn!(
        code = "MCP_HTTP_MALFORMED_JSON",
        detail = %error,
        "HTTP MCP request body rejected: not valid JSON"
    );
    jsonrpc_error_response(
        JSONRPC_PARSE_ERROR,
        "Parse error: request body is not valid JSON",
        serde_json::Value::Null,
        serde_json::json!({
            "reason": "malformed_json",
            "detail": error.to_string(),
        }),
    )
}

/// 400 for valid JSON that cannot be dispatched as a single client message.
pub(super) fn invalid_message_response(error: &serde_json::Error) -> Response {
    tracing::warn!(
        code = "MCP_HTTP_INVALID_MESSAGE",
        detail = %error,
        "HTTP MCP request body rejected: not a single client message"
    );
    jsonrpc_error_response(
        JSONRPC_INVALID_REQUEST,
        "Invalid Request: expected a single JSON-RPC client message",
        serde_json::Value::Null,
        serde_json::json!({"reason": "invalid_message", "detail": error.to_string()}),
    )
}

/// 400 with a JSON-RPC `-32600` invalid-request error for `initialize` on a
/// session that was already initialized; the session keeps its negotiated state.
pub(super) fn already_initialized_response(
    session_id: &str,
    request: &serde_json::Value,
) -> Response {
    tracing::warn!(
        code = "MCP_HTTP_SESSION_ALREADY_INITIALIZED",
        session_id,
        "HTTP MCP initialize rejected: session is already initialized"
    );
    let id = request
        .get("id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    jsonrpc_error_response(
        JSONRPC_INVALID_REQUEST,
        "Invalid Request: session is already initialized",
        id,
        serde_json::json!({
            "reason": "session_already_initialized",
            "session_id": session_id,
            "recovery": "open a new session (POST /mcp without Mcp-Session-Id) to renegotiate",
        }),
    )
}

fn jsonrpc_error_response(
    code: i64,
    message: &str,
    id: serde_json::Value,
    data: serde_json::Value,
) -> Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message, "data": data},
    });
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body.to_string(),
    )
        .into_response()
}
