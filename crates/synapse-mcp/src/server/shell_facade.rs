//! Strict shell schema and per-call owner approval for the shell facade.

use super::{
    ActRunShellCancelResponse, ActRunShellParams, ActRunShellResponse, ActRunShellStartParams,
    ActRunShellStartResponse, ActRunShellStatusResponse, Arc, BTreeMap, ErrorData, Json, Map,
    Parameters, RequestContext, RoleServer, SHELL_FACADE_SOURCE_OF_TRUTH, ShellExecutionContext,
    ShellFacadeResponse, ShellOperation, ShellParams, SynapseService, Value,
    require_shell_session_id, run_shell_request_details, run_shell_start_request_details,
    shell_cancel_params, shell_execution_context_for_session, shell_facade_delegate_error,
    shell_run_params, shell_start_params, shell_status_params, shell_unexpected_fields,
};
use crate::server::command_audit::{CommandAuditInput, command_audit_error_from_error_data};
use crate::server::owner_override::{ApprovedCall, OwnerOverride};
use serde::Serialize;
use serde_json::json;

pub(super) struct ShellAudit {
    tool: &'static str,
    kind: &'static str,
    session_id: String,
    payload: Value,
    before: Value,
}

pub(super) fn input_schema() -> Arc<Map<String, Value>> {
    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("additionalProperties".to_owned(), json!(false));
    schema.insert("properties".to_owned(), properties());
    schema.insert(
        "not".to_owned(),
        json!({"required": ["reason", "owner_approval_reason"]}),
    );
    schema.insert(
        "oneOf".to_owned(),
        json!([
            branch(
                "run",
                &[
                    "command",
                    "args",
                    "working_dir",
                    "env",
                    "timeout_ms",
                    "execution_mode",
                    "durable_timeout_ms",
                    "idempotency_key",
                    "owner_approved",
                    "reason",
                    "owner_approval_reason"
                ]
            ),
            branch(
                "start",
                &[
                    "command",
                    "args",
                    "working_dir",
                    "env",
                    "timeout_ms",
                    "job_id",
                    "owner_approved",
                    "reason",
                    "owner_approval_reason"
                ]
            ),
            branch("status", &["job_id", "tail_bytes"]),
            branch("cancel", &["job_id"])
        ]),
    );
    Arc::new(schema)
}

fn properties() -> Value {
    json!({
        "operation": {"type": "string", "enum": ["run", "start", "status", "cancel"], "default": "run", "description": "Shell facade operation. Omit only for run."},
        "command": {"type": ["string", "null"], "description": "Executable path/name only. Accepted by run/start."},
        "args": {"type": ["array", "null"], "items": {"type": "string"}, "description": "Literal executable arguments for run/start."},
        "working_dir": {"type": ["string", "null"], "description": "Working directory for run/start."},
        "env": {"type": ["object", "null"], "additionalProperties": {"type": "string"}, "description": "Extra environment variables for run/start."},
        "timeout_ms": {"type": ["integer", "null"], "minimum": 1, "description": "run: inline wait budget. start: durable lifetime cap; omitted means unbounded."},
        "execution_mode": {"type": ["string", "null"], "enum": ["auto", "inline", "durable", null], "description": "run only. Controls inline vs durable routing."},
        "durable_timeout_ms": {"type": ["integer", "null"], "minimum": 1, "description": "run only. Lifetime cap when run creates a durable job; start uses timeout_ms."},
        "idempotency_key": {"type": ["string", "null"], "description": "run only. Deduplicates matching run requests."},
        "job_id": {"type": ["string", "null"], "minLength": 1, "maxLength": 128, "description": "Optional for start, required for status/cancel."},
        "tail_bytes": {"type": ["integer", "null"], "minimum": 0, "maximum": 1048576, "description": "status only. stdout/stderr tail bytes."},
        "owner_approved": {"type": ["boolean", "null"], "description": "run/start only. Explicit owner approval of this GUI launch while active; requires a non-empty reason and is audited per call."},
        "reason": {"type": ["string", "null"], "description": "run/start only. Required non-empty when owner_approved=true. Do not combine with owner_approval_reason."},
        "owner_approval_reason": {"type": ["string", "null"], "description": "run/start only. Alias for reason; supply only one spelling."}
    })
}

fn branch(operation: &str, fields: &[&str]) -> Value {
    let mut properties = Map::new();
    let operation_schema = if operation == "run" {
        json!({"type": "string", "const": operation, "default": "run"})
    } else {
        json!({"type": "string", "const": operation})
    };
    properties.insert("operation".to_owned(), operation_schema);
    for field in fields {
        properties.insert(
            (*field).to_owned(),
            json!({"$ref": format!("#/properties/{field}")}),
        );
    }
    let required = match operation {
        "run" => vec!["command"],
        "start" => vec!["operation", "command"],
        _ => vec!["operation", "job_id"],
    };
    json!({
        "title": format!("shell operation={operation}"),
        "type": "object", "additionalProperties": false,
        "required": required, "properties": properties,
    })
}

pub(super) fn owner_approval(
    params: &ShellParams,
    context: &RequestContext<RoleServer>,
) -> Result<Option<ApprovedCall>, ErrorData> {
    if !matches!(
        params.operation,
        ShellOperation::Run | ShellOperation::Start
    ) {
        let mut fields = Vec::new();
        if params.owner_approved.is_some() {
            fields.push("owner_approved");
        }
        if params.reason.is_some() {
            fields.push("reason/owner_approval_reason");
        }
        shell_unexpected_fields(params.operation, params, &fields)?;
        return Ok(None);
    }
    let owner_override = OwnerOverride::from_request(
        "shell operation=run/start",
        params.owner_approved,
        params.reason.as_deref(),
    )?;
    owner_override
        .map(|approved| {
            let session_id = require_shell_session_id(context)?;
            Ok(ApprovedCall::with_override(&session_id, approved))
        })
        .transpose()
}

impl SynapseService {
    pub(super) fn audit_shell_launch_guard(
        &self,
        tool: &'static str,
        result: Result<(), ErrorData>,
        context: &RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        if let Err(error) = &result {
            self.audit_action_denied_for_request(tool, error, context);
        }
        result
    }

    pub(super) fn prepare_shell_context(
        &self,
        tool: &'static str,
        command: &mut String,
        args: &[String],
        env: &BTreeMap<String, String>,
        working_dir: Option<&str>,
        context: &RequestContext<RoleServer>,
    ) -> Result<ShellExecutionContext, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = tool,
            command = command.as_str(),
            "tool.invocation kind={tool}"
        );
        if let Err(error) = self.ensure_supported_use_allows_action("act_run_shell") {
            self.audit_action_denied_for_request(tool, &error, context);
            return Err(error);
        }
        let session_id = require_shell_session_id(context)?;
        if let Err(error) = self.require_shell_command_profile_policy(
            &session_id,
            command.as_str(),
            args,
            !env.is_empty(),
            working_dir.is_some(),
        ) {
            self.audit_action_denied_for_request(tool, &error, context);
            return Err(error);
        }
        // #2360: a read-only command runs the Windows system folder's program,
        // never a same-named file in the working folder or on the PATH.
        if let Some(program) = self.m4_config.read_only_commands().program(
            command.as_str(),
            args,
            !env.is_empty(),
            working_dir.is_some(),
        ) {
            *command = program.to_string_lossy().into_owned();
        }
        shell_execution_context_for_session(&session_id)
    }

    pub(super) fn begin_shell_run_audit(
        &self,
        params: &ActRunShellParams,
        context: &ShellExecutionContext,
    ) -> Result<ShellAudit, ErrorData> {
        self.begin_shell_audit(ShellAudit {
            tool: "act_run_shell", kind: "shell_run", session_id: context.session_id().to_owned(),
            payload: run_shell_request_details(params, self.m4_config.run_shell_inline_await_limit_ms()),
            before: json!({
                "source_of_truth": "durable shell registry/log files or inline child process",
                "session_id": context.session_id(), "execution_mode": params.execution_mode.as_str(),
            }),
        })
    }

    pub(super) fn begin_shell_start_audit(
        &self,
        params: &ActRunShellStartParams,
        context: &ShellExecutionContext,
    ) -> Result<ShellAudit, ErrorData> {
        self.begin_shell_audit(ShellAudit {
            tool: "act_run_shell_start",
            kind: "shell_spawn",
            session_id: context.session_id().to_owned(),
            payload: run_shell_start_request_details(params),
            before: json!({
                "source_of_truth": "durable shell registry/log files/process table",
                "session_id": context.session_id(), "job_id": params.job_id,
            }),
        })
    }

    fn begin_shell_audit(&self, mut audit: ShellAudit) -> Result<ShellAudit, ErrorData> {
        crate::server::owner_override::audit_call_owner_override(
            &mut audit.payload,
            &audit.session_id,
        );
        self.command_audit_intent(CommandAuditInput::mcp(
            audit.tool,
            audit.kind,
            Some(audit.session_id.clone()),
            Some(audit.session_id.clone()),
            audit.payload.clone(),
            audit.before.clone(),
            Value::Null,
            "pending",
        ))?;
        self.audit_action_started_with_details_for_session(
            audit.tool,
            &audit.payload,
            &audit.session_id,
        )?;
        Ok(audit)
    }

    pub(super) fn finish_shell_audit<T: Serialize>(
        &self,
        audit: ShellAudit,
        result: &Result<T, ErrorData>,
    ) -> Result<(), ErrorData> {
        let mut after = json!({"source_of_truth": audit.before["source_of_truth"]});
        if let Ok(response) = result {
            after["response"] = json!(response);
        }
        let mut input = CommandAuditInput::mcp(
            audit.tool,
            audit.kind,
            Some(audit.session_id.clone()),
            Some(audit.session_id.clone()),
            audit.payload,
            audit.before,
            after,
            if result.is_ok() { "ok" } else { "error" },
        );
        if let Err(error) = result {
            input = input.with_error(command_audit_error_from_error_data(error));
        }
        // #198: attempt both closing rows, even if the command audit fails.
        let command_final = self.command_audit_final(input);
        self.audit_action_result_for_session(audit.tool, result, &audit.session_id)?;
        command_final.map(|_| ())
    }

    pub(super) async fn dispatch_shell(
        &self,
        params: ShellParams,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ShellFacadeResponse>, ErrorData> {
        let mut response = ShellFacadeResponse {
            operation: params.operation,
            source_of_truth: SHELL_FACADE_SOURCE_OF_TRUTH.to_owned(),
            run: None,
            start: None,
            status: None,
            cancel: None,
        };
        match params.operation {
            ShellOperation::Run => response.run = Some(self.shell_run(params, context).await?),
            ShellOperation::Start => {
                response.start = Some(self.shell_start(params, context).await?)
            }
            ShellOperation::Status => {
                response.status = Some(self.shell_status(params, context).await?)
            }
            ShellOperation::Cancel => {
                response.cancel = Some(self.shell_cancel(params, context).await?)
            }
        }
        Ok(Json(response))
    }

    async fn shell_run(
        &self,
        params: ShellParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ActRunShellResponse, ErrorData> {
        let params = shell_run_params(params)?;
        let source_id = params.command.clone();
        self.act_run_shell(Parameters(params), context).await.map(|response| response.0)
            .map_err(|error| shell_facade_delegate_error(
                ShellOperation::Run, source_id, error,
                "inspect the shell command policy, executable path, and job artifacts before retrying",
            ))
    }

    async fn shell_start(
        &self,
        params: ShellParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ActRunShellStartResponse, ErrorData> {
        let params = shell_start_params(params)?;
        let source_id = params.command.clone();
        self.act_run_shell_start(Parameters(params), context).await.map(|response| response.0)
            .map_err(|error| shell_facade_delegate_error(
                ShellOperation::Start, source_id, error,
                "inspect the durable shell command policy and job artifact root before retrying",
            ))
    }

    async fn shell_status(
        &self,
        params: ShellParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ActRunShellStatusResponse, ErrorData> {
        let params = shell_status_params(params)?;
        let source_id = params.job_id.clone();
        self.act_run_shell_status(Parameters(params), context).await.map(|response| response.0)
            .map_err(|error| shell_facade_delegate_error(
                ShellOperation::Status, source_id, error,
                "provide an exact job_id owned by this MCP session and inspect the job status file",
            ))
    }

    async fn shell_cancel(
        &self,
        params: ShellParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ActRunShellCancelResponse, ErrorData> {
        let params = shell_cancel_params(params)?;
        let source_id = params.job_id.clone();
        self.act_run_shell_cancel(Parameters(params), context).await.map(|response| response.0)
            .map_err(|error| shell_facade_delegate_error(
                ShellOperation::Cancel, source_id, error,
                "provide an exact live job_id owned by this MCP session and inspect before/after status",
            ))
    }
}
