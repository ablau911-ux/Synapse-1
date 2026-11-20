use super::action_history::{CallEnd, HistoryCall};
use super::{
    ErrorData, Implementation, ServerCapabilities, ServerHandler, ServerInfo, SessionTarget,
    SynapseService, mcp_error, tool_handler,
};
use futures_util::FutureExt as _;
use rmcp::model::{CallToolResult, ErrorCode};
use serde_json::{Value, json};
use std::panic::AssertUnwindSafe;
use synapse_core::error_codes;

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SynapseService {
    async fn initialize(
        &self,
        request: rmcp::model::InitializeRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::InitializeResult, ErrorData> {
        let mut peers = self
            .initialized_peers
            .lock()
            .map_err(|_error| ErrorData::internal_error("initialize state lock poisoned", None))?;
        peers.retain(|peer| !peer.is_transport_closed());
        // rmcp seeds peer_info BEFORE the first initialize callback. Its stable
        // allocation identifies this connection across service/peer clones.
        if peers.iter().any(|peer| {
            peer.peer_info()
                .zip(context.peer.peer_info())
                .is_some_and(|(first, current)| std::ptr::eq(first, current))
        }) {
            return Err(ErrorData::new(
                ErrorCode::INVALID_REQUEST,
                "Invalid Request: session is already initialized",
                Some(json!({"reason": "session_already_initialized"})),
            ));
        }
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        peers.push(context.peer);
        // rmcp negotiates the first response against the client's version.
        Ok(self.get_info())
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, ErrorData> {
        let tool_name = request.name.to_string();
        let mcp_session_id = super::context::mcp_session_id_from_request_context(&context)?;
        let lifecycle_guard =
            self.begin_daemon_lifecycle_tool_call(&tool_name, mcp_session_id.as_deref());
        if let Some(session_id) = mcp_session_id.as_deref()
            && let Err(error) = self.reject_terminated_session_tool_call(&tool_name, session_id)
        {
            settle_lifecycle(
                &tool_name,
                lifecycle_guard.finish_error(error_snapshot(&error)),
            );
            return Err(error);
        }
        let admission = match mcp_session_id.as_deref() {
            Some(_) => self.admit_tool_call_for_profile(&tool_name, mcp_session_id.as_deref()),
            None => self.admit_sessionless_tool_call(&tool_name),
        };
        if let Err(error) = admission {
            if profile_policy_denied(&error) {
                match context.peer.notify_tool_list_changed().await {
                    Ok(()) => {
                        tracing::info!(
                            code = "MCP_TOOL_LIST_CHANGED_NOTIFIED",
                            tool = %tool_name,
                            mcp_session_id = ?mcp_session_id,
                            "profile policy denied a hidden/stale tool call and pushed notifications/tools/list_changed"
                        );
                    }
                    Err(notify_err) => {
                        tracing::error!(
                            code = "MCP_TOOL_LIST_CHANGED_NOTIFY_FAILED",
                            tool = %tool_name,
                            mcp_session_id = ?mcp_session_id,
                            error = %notify_err,
                            "profile policy denied a hidden/stale tool call but failed to notify tools/list_changed"
                        );
                    }
                }
            }
            settle_lifecycle(
                &tool_name,
                lifecycle_guard.finish_error(error_snapshot(&error)),
            );
            return Err(error);
        }
        let shutdown_cancel = self.shutdown_cancel_token()?;
        let drain_state = self.drain_state_handle();
        let drain_cancel = drain_state.token();
        let drain_snapshot = drain_state.snapshot();
        if drain_snapshot.draining || drain_cancel.is_cancelled() || shutdown_cancel.is_cancelled()
        {
            let snapshot = if drain_snapshot.draining {
                drain_snapshot
            } else {
                drain_state.mark_draining("shutdown_token")
            };
            let error =
                daemon_restarting_mcp_error(&tool_name, mcp_session_id.as_deref(), snapshot);
            settle_lifecycle(
                &tool_name,
                lifecycle_guard.finish_error(error_snapshot(&error)),
            );
            return Err(error);
        }
        // #299: `agent_id` selects which agent of this session the call runs
        // as; the tool's own parameters never see it.
        let mut request = request;
        let agent_scope = match super::agent_scope::scope_for_call(
            &tool_name,
            mcp_session_id.as_deref(),
            &mut request.arguments,
        ) {
            Ok(agent_scope) => agent_scope,
            Err(error) => {
                settle_lifecycle(
                    &tool_name,
                    lifecycle_guard.finish_error(error_snapshot(&error)),
                );
                return Err(error);
            }
        };
        // Phase 7 part 4: every finished call of a tool with mutating
        // operations is offered to the action history (one hook, here).
        let history_call = HistoryCall::start(
            &tool_name,
            request.arguments.as_ref(),
            self.tool_router.get(&tool_name),
        );
        let context = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let tool_call = AssertUnwindSafe(super::agent_scope::AgentScope::scope(
            agent_scope,
            self.tool_router.call(context),
        ))
        .catch_unwind();
        tokio::pin!(tool_call);
        let result = tokio::select! {
            _ = drain_cancel.cancelled() => {
                let snapshot = drain_state.mark_draining("drain_token");
                let error = daemon_restarting_mcp_error(&tool_name, mcp_session_id.as_deref(), snapshot);
                settle_lifecycle(&tool_name, lifecycle_guard.finish_error(error_snapshot(&error)));
                return Err(error);
            }
            _ = shutdown_cancel.cancelled() => {
                let snapshot = drain_state.mark_draining("shutdown_token");
                let error = daemon_restarting_mcp_error(&tool_name, mcp_session_id.as_deref(), snapshot);
                settle_lifecycle(&tool_name, lifecycle_guard.finish_error(error_snapshot(&error)));
                return Err(error);
            }
            result = &mut tool_call => result,
        };
        match result {
            Ok(Ok(mut result)) => {
                // #10: an outer success can wrap a refused/failed delegated
                // action; the lifecycle ledger records that as an error, and
                // the MCP result is flagged isError=true (#221).
                let (nested_failure, effective_target) =
                    classify_tool_success(&tool_name, &mut result);
                if let Some(call) = history_call {
                    let end = CallEnd {
                        reply_operation: result
                            .structured_content
                            .as_ref()
                            .and_then(|content| content.get("operation"))
                            .and_then(Value::as_str),
                        failed: nested_failure.is_some(),
                        error_code: nested_failure
                            .as_ref()
                            .and_then(|failure| failure.code.as_deref()),
                        effective_target: effective_target.as_ref(),
                    };
                    self.record_action_history(call, mcp_session_id.as_deref(), &end);
                }
                match nested_failure {
                    Some(failure) => settle_lifecycle(
                        &tool_name,
                        lifecycle_guard.finish_error_with_effective_target(
                            failure.error_snapshot(),
                            effective_target,
                        ),
                    ),
                    None => settle_lifecycle(
                        &tool_name,
                        lifecycle_guard.finish_ok_with_effective_target(effective_target),
                    ),
                }
                Ok(result)
            }
            Ok(Err(error)) => {
                let error = normalize_tool_error(&tool_name, error);
                let error_snapshot = error_snapshot(&error);
                let effective_target = effective_target_from_error_snapshot(&error_snapshot);
                if let Some(call) = history_call {
                    let end = CallEnd {
                        reply_operation: error_data_text(&error, "operation"),
                        failed: true,
                        error_code: error_data_text(&error, "code"),
                        effective_target: effective_target.as_ref(),
                    };
                    self.record_action_history(call, mcp_session_id.as_deref(), &end);
                }
                settle_lifecycle(
                    &tool_name,
                    lifecycle_guard
                        .finish_error_with_effective_target(error_snapshot, effective_target),
                );
                Err(error)
            }
            Err(payload) => {
                let panic_message =
                    crate::daemon_lifecycle::panic_payload_message(payload.as_ref());
                std::mem::forget(payload);
                if let Some(call) = history_call {
                    let end = CallEnd {
                        reply_operation: None,
                        failed: true,
                        error_code: Some(error_codes::TOOL_INTERNAL_ERROR),
                        effective_target: None,
                    };
                    self.record_action_history(call, mcp_session_id.as_deref(), &end);
                }
                let panic = json!({
                    "payload": panic_message,
                    "tool": tool_name,
                    "mcp_session_id": mcp_session_id.clone(),
                });
                settle_lifecycle(&tool_name, lifecycle_guard.finish_panic(panic));
                Err(tool_panic_mcp_error(&tool_name, mcp_session_id.as_deref()))
            }
        }
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        let mcp_session_id = super::context::mcp_session_id_from_request_context(&context)?;
        // Normalize schemas before they reach the client, then apply the
        // session's durable tool profile. The policy gate in `call_tool` uses
        // the same profile row so hand-written calls cannot bypass discovery.
        let tools = self.tools_for_session_profile(mcp_session_id.as_deref())?;
        Ok(rmcp::model::ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new(
            "synapse-mcp",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(self.instructions())
    }
}

impl SynapseService {
    fn reject_terminated_session_tool_call(
        &self,
        tool_name: &str,
        session_id: &str,
    ) -> Result<(), ErrorData> {
        let terminated = self.terminated_sessions.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "terminated-session registry lock poisoned while admitting MCP tool call",
            )
        })?;
        if !terminated.contains(session_id) {
            return Ok(());
        }
        tracing::warn!(
            code = error_codes::HTTP_SESSION_INVALID,
            session_id,
            tool = tool_name,
            "MCP tool call rejected because session lifecycle already terminated it"
        );
        Err(ErrorData::new(
            ErrorCode(-32099),
            format!("MCP session {session_id:?} was terminated and cannot call {tool_name}"),
            Some(json!({
                "code": error_codes::HTTP_SESSION_INVALID,
                "session_id": session_id,
                "tool": tool_name,
                "reason": "session_terminated",
                "source_of_truth": "terminated-session registry",
            })),
        ))
    }

    fn begin_daemon_lifecycle_tool_call(
        &self,
        tool_name: &str,
        mcp_session_id: Option<&str>,
    ) -> crate::daemon_lifecycle::ToolCallGuard {
        // #172: diagnostics never refuse a tool call; any failure below is
        // recorded in the ledger's read-error fields or logged (rate-limited).
        let (audit_context, audit_context_read_error) = match self.current_action_audit_context() {
            Ok(context) => ledger_value("audit_context", &context),
            Err(error) => (None, Some(error_snapshot(&error))),
        };
        let (foreground, foreground_read_error) = match self.current_audit_foreground() {
            Ok(foreground) => ledger_value("foreground", &foreground),
            Err(error) => (None, Some(error_snapshot(&error))),
        };
        let (session_target, session_target_read_error) = match self.session_target(mcp_session_id)
        {
            Ok(target) => (target.as_ref().map(session_target_value), None),
            Err(error) => (None, Some(error_snapshot(&error))),
        };
        crate::daemon_lifecycle::begin_tool_call_best_effort(
            crate::daemon_lifecycle::ToolCallStart {
                tool: tool_name.to_owned(),
                mcp_session_id: mcp_session_id.map(ToOwned::to_owned),
                audit_context,
                audit_context_read_error,
                foreground,
                foreground_read_error,
                session_target,
                session_target_read_error,
            },
        )
    }
}

/// Serializes a ledger field; a failure becomes the field's read-error value
/// instead of an error for the tool call (#172).
fn ledger_value<T: serde::Serialize>(field: &str, value: &T) -> (Option<Value>, Option<Value>) {
    match serde_json::to_value(value) {
        Ok(value) => (Some(value), None),
        Err(error) => (
            None,
            Some(json!({"serialize_error": format!("{field}: {error}")})),
        ),
    }
}

/// Finishing the ledger entry is best effort: the tool already ran, so a
/// ledger failure is logged (rate-limited) and the real result is returned.
fn settle_lifecycle(tool_name: &str, result: anyhow::Result<()>) {
    if let Err(error) = result {
        crate::daemon_lifecycle::report_ledger_failure("finish_tool_call", tool_name, &error);
    }
}

fn normalize_tool_error(tool_name: &str, error: ErrorData) -> ErrorData {
    if error.data.is_none() && error.message == "tool not found" {
        return mcp_error(
            synapse_core::error_codes::TOOL_NOT_FOUND,
            format!("tool not found: {tool_name}"),
        );
    }
    if error.data.is_none() && error.code == rmcp::model::ErrorCode::INVALID_PARAMS {
        let message = super::param_hints::param_hint_message(tool_name, &error.message)
            .unwrap_or_else(|| error.message.to_string());
        return mcp_error(synapse_core::error_codes::TOOL_PARAMS_INVALID, message);
    }
    error
}

fn session_target_value(target: &SessionTarget) -> Value {
    match target {
        SessionTarget::Window { hwnd } => json!({
            "kind": "window",
            "hwnd": hwnd,
        }),
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => json!({
            "kind": "cdp",
            "window_hwnd": window_hwnd,
            "cdp_target_id": cdp_target_id,
        }),
    }
}

/// A text field of the error's structured data (`code`, `operation`).
fn error_data_text<'a>(error: &'a ErrorData, field: &str) -> Option<&'a str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get(field))
        .and_then(Value::as_str)
}

fn error_snapshot(error: &ErrorData) -> Value {
    json!({
        "rmcp_code": error.code.0,
        "message": error.message.to_string(),
        "data": error.data.clone(),
        "synapse_code": error.data.as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str),
    })
}

/// Result-to-`isError` mapping for a tool call that returned `Ok(result)`
/// (#10/#221). Reads the effective target from the unmodified result, then
/// flags an outer success that wraps a refused/failed delegated action
/// `isError=true` and returns that failure (`None` = a clean success; the
/// result is left unchanged).
pub(super) fn classify_tool_success(
    tool_name: &str,
    result: &mut CallToolResult,
) -> (
    Option<super::act_outcome::NestedActionFailure>,
    Option<Value>,
) {
    let effective_target = effective_target_from_tool_result(result);
    let nested_failure = super::act_outcome::mark_nested_failure(tool_name, result);
    (nested_failure, effective_target)
}

fn effective_target_from_tool_result(result: &CallToolResult) -> Option<Value> {
    result
        .structured_content
        .as_ref()
        .and_then(|value| effective_target_from_value(value, "structured_content"))
}

fn effective_target_from_error_snapshot(error: &Value) -> Option<Value> {
    let data = error.get("data")?;
    effective_target_from_value(data, "error.data").or_else(|| {
        data.get("source_id")
            .and_then(Value::as_str)
            .and_then(|source_id| {
                effective_target_from_source_id(source_id, "error.data.source_id")
            })
    })
}

fn effective_target_from_value(value: &Value, source: &'static str) -> Option<Value> {
    let object = value.as_object()?;
    if let Some(target) = effective_target_from_fields(object, source) {
        return Some(target);
    }
    if let Some(target) = object
        .get("target")
        .and_then(|value| effective_target_from_value(value, "structured_content.target"))
    {
        return Some(target);
    }
    for (field, nested_source) in [
        ("content", "structured_content.content"),
        ("locate", "structured_content.locate"),
        ("inspect", "structured_content.inspect"),
        ("aria_snapshot", "structured_content.aria_snapshot"),
        ("capture", "structured_content.capture"),
        ("current", "structured_content.current"),
    ] {
        if let Some(target) = object
            .get(field)
            .and_then(|value| effective_target_from_value(value, nested_source))
        {
            return Some(target);
        }
    }
    None
}

fn effective_target_from_fields(
    object: &serde_json::Map<String, Value>,
    source: &'static str,
) -> Option<Value> {
    let cdp_target_id = object
        .get("cdp_target_id")
        .or_else(|| object.get("cdpTargetId"))
        .and_then(Value::as_str)
        .filter(|target_id| !target_id.trim().is_empty());
    let window_hwnd = object
        .get("window_hwnd")
        .or_else(|| object.get("windowHwnd"))
        .and_then(Value::as_i64);
    match (window_hwnd, cdp_target_id) {
        (Some(window_hwnd), Some(cdp_target_id)) => Some(json!({
            "kind": "cdp",
            "window_hwnd": window_hwnd,
            "cdp_target_id": cdp_target_id,
            "source": source,
        })),
        (None, Some(cdp_target_id)) => Some(json!({
            "kind": "cdp",
            "cdp_target_id": cdp_target_id,
            "source": source,
        })),
        (Some(hwnd), None) => Some(json!({
            "kind": "window",
            "hwnd": hwnd,
            "source": source,
        })),
        (None, None) => None,
    }
}

fn effective_target_from_source_id(source_id: &str, source: &'static str) -> Option<Value> {
    let mut window_hwnd = None;
    let mut cdp_target_id = None;
    for part in source_id.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.trim() {
            "window_hwnd" => window_hwnd = parse_hwnd_literal(value.trim()),
            "cdp_target_id" => {
                let target = value.trim();
                if !target.is_empty() {
                    cdp_target_id = Some(target.to_owned());
                }
            }
            _ => {}
        }
    }
    match (window_hwnd, cdp_target_id) {
        (Some(window_hwnd), Some(cdp_target_id)) => Some(json!({
            "kind": "cdp",
            "window_hwnd": window_hwnd,
            "cdp_target_id": cdp_target_id,
            "source": source,
        })),
        (None, Some(cdp_target_id)) => Some(json!({
            "kind": "cdp",
            "cdp_target_id": cdp_target_id,
            "source": source,
        })),
        (Some(hwnd), None) => Some(json!({
            "kind": "window",
            "hwnd": hwnd,
            "source": source,
        })),
        (None, None) => None,
    }
}

fn parse_hwnd_literal(value: &str) -> Option<i64> {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .and_then(|hex| i64::from_str_radix(hex, 16).ok())
        .or_else(|| value.parse::<i64>().ok())
}

fn profile_policy_denied(error: &ErrorData) -> bool {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        == Some(error_codes::TOOL_PROFILE_POLICY_DENIED)
}

fn daemon_restarting_mcp_error(
    tool_name: &str,
    mcp_session_id: Option<&str>,
    snapshot: super::drain::DaemonDrainSnapshot,
) -> ErrorData {
    ErrorData::new(
        rmcp::model::ErrorCode(-32099),
        format!("daemon is restarting; tool {tool_name} was refused before transport shutdown"),
        Some(json!({
            "code": synapse_core::error_codes::DAEMON_RESTARTING,
            "retryable": true,
            "tool": tool_name,
            "mcp_session_id": mcp_session_id,
            "drain": snapshot,
        })),
    )
}

fn tool_panic_mcp_error(tool_name: &str, mcp_session_id: Option<&str>) -> ErrorData {
    ErrorData::new(
        rmcp::model::ErrorCode(-32099),
        format!("tool {tool_name} panicked; daemon lifecycle ledger captured the panic"),
        Some(json!({
            "code": synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            "detail_code": "MCP_TOOL_PANIC",
            "tool": tool_name,
            "mcp_session_id": mcp_session_id,
            "daemon_lifecycle": crate::daemon_lifecycle::diagnostic_value(),
        })),
    )
}

#[cfg(test)]
#[path = "tests/facade_failure_variants_221.rs"]
mod facade_failure_variants_221;
