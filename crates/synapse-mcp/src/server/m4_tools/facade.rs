// Split-out sibling of the m4_tools module; the globs mirror the pre-split
// single-module layout and keep the shared symbol set in one place.
#[allow(clippy::wildcard_imports)]
use super::types::*;
#[allow(clippy::wildcard_imports)]
use super::*;
use crate::hex::hex_encode;

pub(super) fn shell_facade_error(
    operation: ShellOperation,
    source_id: impl Into<String>,
    message: impl Into<String>,
    remediation: &'static str,
) -> ErrorData {
    let message = message.into();
    ErrorData::new(
        ErrorCode(-32099),
        message,
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "operation": operation.as_str(),
            "source_of_truth": SHELL_FACADE_SOURCE_OF_TRUTH,
            "source_id": source_id.into(),
            "remediation": remediation,
        })),
    )
}

pub(super) fn process_facade_error(
    operation: ProcessOperation,
    source_id: impl Into<String>,
    message: impl Into<String>,
    remediation: &'static str,
) -> ErrorData {
    let message = message.into();
    ErrorData::new(
        ErrorCode(-32099),
        message,
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "operation": operation.as_str(),
            "source_of_truth": PROCESS_FACADE_SOURCE_OF_TRUTH,
            "source_id": source_id.into(),
            "remediation": remediation,
        })),
    )
}

pub(super) fn shell_facade_delegate_error(
    operation: ShellOperation,
    source_id: impl Into<String>,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    crate::server::delegate_errors::operation_delegate_error(
        operation.as_str(),
        SHELL_FACADE_SOURCE_OF_TRUTH,
        None,
        source_id,
        error,
        remediation,
    )
}

pub(super) fn process_facade_delegate_error(
    operation: ProcessOperation,
    source_id: impl Into<String>,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    crate::server::delegate_errors::operation_delegate_error(
        operation.as_str(),
        PROCESS_FACADE_SOURCE_OF_TRUTH,
        None,
        source_id,
        error,
        remediation,
    )
}

pub(super) fn require_shell_text(
    operation: ShellOperation,
    value: Option<String>,
    field: &'static str,
    source_id: &str,
) -> Result<String, ErrorData> {
    let Some(value) = value else {
        return Err(shell_facade_error(
            operation,
            source_id,
            format!("shell operation={} requires {field}", operation.as_str()),
            "provide the required field for this shell operation",
        ));
    };
    if value.trim().is_empty() {
        return Err(shell_facade_error(
            operation,
            source_id,
            format!(
                "shell operation={} requires non-empty {field}",
                operation.as_str()
            ),
            "provide a non-empty executable or job id",
        ));
    }
    Ok(value)
}

pub(super) fn require_process_text(
    operation: ProcessOperation,
    value: Option<String>,
    field: &'static str,
    source_id: &str,
) -> Result<String, ErrorData> {
    let Some(value) = value else {
        return Err(process_facade_error(
            operation,
            source_id,
            format!("process operation={} requires {field}", operation.as_str()),
            "provide the required field for this process operation",
        ));
    };
    if value.trim().is_empty() {
        return Err(process_facade_error(
            operation,
            source_id,
            format!(
                "process operation={} requires non-empty {field}",
                operation.as_str()
            ),
            "provide a non-empty executable path/name or source id",
        ));
    }
    Ok(value)
}

pub(super) fn shell_unexpected_fields(
    operation: ShellOperation,
    params: &ShellParams,
    fields: &[&'static str],
) -> Result<(), ErrorData> {
    if fields.is_empty() {
        return Ok(());
    }
    let source_id = params
        .job_id
        .as_deref()
        .or(params.command.as_deref())
        .unwrap_or_else(|| operation.as_str());
    Err(shell_facade_error(
        operation,
        source_id,
        format!(
            "shell operation={} does not accept field(s): {}",
            operation.as_str(),
            fields.join(", ")
        ),
        "remove fields that belong to a different shell operation",
    ))
}

pub(super) fn process_unexpected_fields(
    operation: ProcessOperation,
    params: &ProcessParams,
    fields: &[&'static str],
) -> Result<(), ErrorData> {
    if fields.is_empty() {
        return Ok(());
    }
    let source_id = params
        .target
        .as_deref()
        .or_else(|| params.process_name_contains.as_deref())
        .or_else(|| params.command_line_contains.as_deref())
        .map(str::to_owned)
        .or_else(|| params.pid.map(|pid| pid.to_string()))
        .unwrap_or_else(|| operation.as_str().to_owned());
    Err(process_facade_error(
        operation,
        source_id,
        format!(
            "process operation={} does not accept field(s): {}",
            operation.as_str(),
            fields.join(", ")
        ),
        "remove fields that belong to a different process operation",
    ))
}

pub(super) fn shell_run_params(params: ShellParams) -> Result<ActRunShellParams, ErrorData> {
    let mut unexpected = Vec::new();
    if params.job_id.is_some() {
        unexpected.push("job_id");
    }
    if params.tail_bytes.is_some() {
        unexpected.push("tail_bytes");
    }
    shell_unexpected_fields(ShellOperation::Run, &params, &unexpected)?;
    let command = require_shell_text(
        ShellOperation::Run,
        params.command,
        "command",
        ShellOperation::Run.as_str(),
    )?;
    Ok(ActRunShellParams {
        command,
        args: params.args.unwrap_or_default(),
        working_dir: params.working_dir,
        env: params.env.unwrap_or_default(),
        timeout_ms: params
            .timeout_ms
            .unwrap_or(crate::m4::DEFAULT_SHELL_TIMEOUT_MS),
        execution_mode: params
            .execution_mode
            .unwrap_or(ActRunShellExecutionMode::Auto),
        durable_timeout_ms: params.durable_timeout_ms,
        idempotency_key: params.idempotency_key,
    })
}

pub(super) fn shell_start_params(params: ShellParams) -> Result<ActRunShellStartParams, ErrorData> {
    let mut unexpected = Vec::new();
    if params.execution_mode.is_some() {
        unexpected.push("execution_mode");
    }
    if params.durable_timeout_ms.is_some() {
        unexpected.push("durable_timeout_ms");
    }
    if params.idempotency_key.is_some() {
        unexpected.push("idempotency_key");
    }
    if params.tail_bytes.is_some() {
        unexpected.push("tail_bytes");
    }
    shell_unexpected_fields(ShellOperation::Start, &params, &unexpected)?;
    let command = require_shell_text(
        ShellOperation::Start,
        params.command,
        "command",
        ShellOperation::Start.as_str(),
    )?;
    Ok(ActRunShellStartParams {
        command,
        args: params.args.unwrap_or_default(),
        working_dir: params.working_dir,
        env: params.env.unwrap_or_default(),
        timeout_ms: params.timeout_ms,
        job_id: params.job_id,
    })
}

pub(super) fn shell_status_params(
    params: ShellParams,
) -> Result<ActRunShellStatusParams, ErrorData> {
    let mut unexpected = Vec::new();
    if params.command.is_some() {
        unexpected.push("command");
    }
    if params.args.is_some() {
        unexpected.push("args");
    }
    if params.working_dir.is_some() {
        unexpected.push("working_dir");
    }
    if params.env.is_some() {
        unexpected.push("env");
    }
    if params.timeout_ms.is_some() {
        unexpected.push("timeout_ms");
    }
    if params.execution_mode.is_some() {
        unexpected.push("execution_mode");
    }
    if params.durable_timeout_ms.is_some() {
        unexpected.push("durable_timeout_ms");
    }
    if params.idempotency_key.is_some() {
        unexpected.push("idempotency_key");
    }
    shell_unexpected_fields(ShellOperation::Status, &params, &unexpected)?;
    let job_id = require_shell_text(
        ShellOperation::Status,
        params.job_id,
        "job_id",
        ShellOperation::Status.as_str(),
    )?;
    Ok(ActRunShellStatusParams {
        job_id,
        tail_bytes: params
            .tail_bytes
            .unwrap_or(crate::m4::SHELL_JOB_TAIL_DEFAULT_BYTES),
    })
}

pub(super) fn shell_cancel_params(
    params: ShellParams,
) -> Result<ActRunShellJobIdParams, ErrorData> {
    let mut unexpected = Vec::new();
    if params.command.is_some() {
        unexpected.push("command");
    }
    if params.args.is_some() {
        unexpected.push("args");
    }
    if params.working_dir.is_some() {
        unexpected.push("working_dir");
    }
    if params.env.is_some() {
        unexpected.push("env");
    }
    if params.timeout_ms.is_some() {
        unexpected.push("timeout_ms");
    }
    if params.execution_mode.is_some() {
        unexpected.push("execution_mode");
    }
    if params.durable_timeout_ms.is_some() {
        unexpected.push("durable_timeout_ms");
    }
    if params.idempotency_key.is_some() {
        unexpected.push("idempotency_key");
    }
    if params.tail_bytes.is_some() {
        unexpected.push("tail_bytes");
    }
    shell_unexpected_fields(ShellOperation::Cancel, &params, &unexpected)?;
    let job_id = require_shell_text(
        ShellOperation::Cancel,
        params.job_id,
        "job_id",
        ShellOperation::Cancel.as_str(),
    )?;
    Ok(ActRunShellJobIdParams { job_id })
}

pub(super) fn process_launch_params(params: ProcessParams) -> Result<ActLaunchParams, ErrorData> {
    let mut unexpected = Vec::new();
    if params.pid.is_some() {
        unexpected.push("pid");
    }
    if params.process_name_contains.is_some() {
        unexpected.push("process_name_contains");
    }
    if params.command_line_contains.is_some() {
        unexpected.push("command_line_contains");
    }
    if params.limit.is_some() {
        unexpected.push("limit");
    }
    if params.include_command_line.is_some() {
        unexpected.push("include_command_line");
    }
    process_unexpected_fields(ProcessOperation::Launch, &params, &unexpected)?;
    let target = require_process_text(
        ProcessOperation::Launch,
        params.target,
        "target",
        ProcessOperation::Launch.as_str(),
    )?;
    Ok(ActLaunchParams {
        target,
        args: params.args.unwrap_or_default(),
        working_dir: params.working_dir,
        env: params.env.unwrap_or_default(),
        wait_for_window_title_regex: params.wait_for_window_title_regex,
        timeout_ms: params
            .timeout_ms
            .unwrap_or(crate::m4::DEFAULT_LAUNCH_TIMEOUT_MS),
        idempotency_key: params.idempotency_key,
        cdp_debug: params.cdp_debug,
        force_renderer_accessibility: params.force_renderer_accessibility,
        windows_console_window_state: params.windows_console_window_state,
        desktop: params.desktop,
    })
}

pub(super) fn validate_process_query_params(
    operation: ProcessOperation,
    params: &ProcessParams,
) -> Result<usize, ErrorData> {
    let mut unexpected = Vec::new();
    if params.target.is_some() {
        unexpected.push("target");
    }
    if params.args.is_some() {
        unexpected.push("args");
    }
    if params.working_dir.is_some() {
        unexpected.push("working_dir");
    }
    if params.env.is_some() {
        unexpected.push("env");
    }
    if params.wait_for_window_title_regex.is_some() {
        unexpected.push("wait_for_window_title_regex");
    }
    if params.timeout_ms.is_some() {
        unexpected.push("timeout_ms");
    }
    if params.idempotency_key.is_some() {
        unexpected.push("idempotency_key");
    }
    if params.cdp_debug.is_some() {
        unexpected.push("cdp_debug");
    }
    if params.force_renderer_accessibility.is_some() {
        unexpected.push("force_renderer_accessibility");
    }
    if params.windows_console_window_state.is_some() {
        unexpected.push("windows_console_window_state");
    }
    if params.desktop.is_some() {
        unexpected.push("desktop");
    }
    process_unexpected_fields(operation, params, &unexpected)?;

    for (field, value) in [
        (
            "process_name_contains",
            params.process_name_contains.as_deref(),
        ),
        (
            "command_line_contains",
            params.command_line_contains.as_deref(),
        ),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            return Err(process_facade_error(
                operation,
                field,
                format!(
                    "process operation={} requires non-empty {field}",
                    operation.as_str()
                ),
                "remove the empty filter or provide a non-empty filter value",
            ));
        }
    }

    let limit = params.limit.unwrap_or(match operation {
        ProcessOperation::List => PROCESS_LIST_DEFAULT_LIMIT,
        ProcessOperation::History => PROCESS_HISTORY_DEFAULT_LIMIT,
        ProcessOperation::Launch | ProcessOperation::Terminate => {
            unreachable!("launch/terminate are not query operations")
        }
    });
    let max_limit = match operation {
        ProcessOperation::List => PROCESS_LIST_MAX_LIMIT,
        ProcessOperation::History => PROCESS_HISTORY_MAX_LIMIT,
        ProcessOperation::Launch | ProcessOperation::Terminate => {
            unreachable!("launch/terminate are not query operations")
        }
    };
    if limit == 0 || limit > max_limit {
        return Err(process_facade_error(
            operation,
            limit.to_string(),
            format!(
                "process operation={} limit must be between 1 and {max_limit}",
                operation.as_str()
            ),
            "use a bounded positive limit for the requested readback",
        ));
    }
    Ok(limit)
}

pub(super) fn process_filters(params: &ProcessParams) -> ProcessFilters {
    ProcessFilters {
        pid: params.pid,
        process_name_contains: params.process_name_contains.clone(),
        command_line_contains: params.command_line_contains.clone(),
    }
}

pub(super) fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

pub(super) fn process_row_matches(filters: &ProcessFilters, row: &ProcessRow) -> bool {
    if filters.pid.is_some_and(|pid| row.pid != pid) {
        return false;
    }
    if let Some(filter) = filters.process_name_contains.as_deref()
        && !contains_case_insensitive(&row.name, filter)
    {
        return false;
    }
    if let Some(filter) = filters.command_line_contains.as_deref() {
        let Some(command_line) = row.command_line.as_deref() else {
            return false;
        };
        if !contains_case_insensitive(command_line, filter) {
            return false;
        }
    }
    true
}

pub(super) fn process_history_row_matches(
    filters: &ProcessFilters,
    row: &ProcessHistoryRow,
) -> bool {
    if filters.pid.is_some_and(|pid| row.pid != Some(pid)) {
        return false;
    }
    if let Some(filter) = filters.process_name_contains.as_deref() {
        let Some(target) = row.target.as_deref() else {
            return false;
        };
        if !contains_case_insensitive(target, filter) {
            return false;
        }
    }
    if let Some(filter) = filters.command_line_contains.as_deref() {
        let Some(command_line) = row.command_line.as_deref() else {
            return false;
        };
        if !contains_case_insensitive(command_line, filter) {
            return false;
        }
    }
    true
}

pub(super) fn process_list_response(
    params: &ProcessParams,
) -> Result<ProcessListResponse, ErrorData> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let limit = validate_process_query_params(ProcessOperation::List, params)?;
    let filters = process_filters(params);
    let include_command_line =
        params.include_command_line.unwrap_or(false) || filters.command_line_contains.is_some();
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );

    let mut rows = Vec::new();
    for (pid, process) in system.processes() {
        let command_line = process
            .cmd()
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let row = ProcessRow {
            pid: pid.as_u32(),
            parent_pid: process.parent().map(|parent| parent.as_u32()),
            name: process.name().to_string_lossy().into_owned(),
            exe: process.exe().map(|path| path.display().to_string()),
            cwd: process.cwd().map(|path| path.display().to_string()),
            status: format!("{:?}", process.status()),
            start_time_unix_ms: process.start_time().saturating_mul(1000),
            command_line: include_command_line.then_some(command_line),
        };
        if !process_row_matches(&filters, &row) {
            continue;
        }
        rows.push(row);
        if rows.len() >= limit {
            break;
        }
    }
    Ok(ProcessListResponse {
        source_of_truth: "live OS process table via sysinfo refresh_processes_specifics".to_owned(),
        returned_count: rows.len(),
        limit,
        filters,
        rows,
    })
}

pub(super) fn process_history_response(
    service: &SynapseService,
    params: &ProcessParams,
) -> Result<ProcessHistoryResponse, ErrorData> {
    let limit = validate_process_query_params(ProcessOperation::History, params)?;
    let filters = process_filters(params);
    let rows = {
        let runtime = service.reflex_runtime()?;
        let runtime = runtime.lock().map_err(|_error| {
            process_facade_error(
                ProcessOperation::History,
                cf::CF_PROCESS_HISTORY,
                "reflex runtime lock poisoned while reading process history",
                "retry after the daemon lock recovers; inspect daemon logs if this repeats",
            )
        })?;
        runtime
            .storage_cf_tail_rows(cf::CF_PROCESS_HISTORY, limit)
            .map_err(|error| {
                process_facade_error(
                    ProcessOperation::History,
                    cf::CF_PROCESS_HISTORY,
                    format!("CF_PROCESS_HISTORY tail read failed: {error}"),
                    "inspect the RocksDB column family and daemon storage logs",
                )
            })?
    };
    let scanned_tail_rows = rows.len();
    let mut decoded_rows = Vec::new();
    for (key, value) in rows {
        let decoded = decode_json::<Value>(&value).map_err(|error| {
            process_facade_error(
                ProcessOperation::History,
                hex_encode(&key),
                format!("CF_PROCESS_HISTORY row decode failed: {error}"),
                "inspect the exact process history row bytes and fix the writer",
            )
        })?;
        let row_json = serde_json::to_string(&decoded).map_err(|error| {
            process_facade_error(
                ProcessOperation::History,
                hex_encode(&key),
                format!("CF_PROCESS_HISTORY row JSON render failed: {error}"),
                "inspect the decoded process history row",
            )
        })?;
        let row = ProcessHistoryRow {
            key: String::from_utf8_lossy(&key).into_owned(),
            key_hex: hex_encode(&key),
            value_len_bytes: u64::try_from(value.len()).unwrap_or(u64::MAX),
            row_json,
            pid: json_u32_field(&decoded, "pid"),
            target: json_string_field(&decoded, "target"),
            tool: json_string_field(&decoded, "tool"),
            status: json_string_field(&decoded, "status"),
            launched_at: json_string_field(&decoded, "launched_at"),
            command_line: json_string_field(&decoded, "command_line"),
        };
        if process_history_row_matches(&filters, &row) {
            decoded_rows.push(row);
        }
    }
    Ok(ProcessHistoryResponse {
        source_of_truth: PROCESS_FACADE_SOURCE_OF_TRUTH.to_owned(),
        cf_name: cf::CF_PROCESS_HISTORY.to_owned(),
        returned_count: decoded_rows.len(),
        scanned_tail_rows,
        limit,
        filters,
        rows: decoded_rows,
    })
}

const PROCESS_TERMINATE_DEFAULT_GRACE_MS: u64 = 5_000;
const PROCESS_TERMINATE_MAX_GRACE_MS: u64 = 30_000;
const PROCESS_TERMINATE_SOURCE_OF_TRUTH: &str = "session process-resource ledger (ownership) + launch-time Windows job object (ownership proof, membership and exit readback)";
/// Poll interval of the job-empty readback during the graceful and forced waits.
const PROCESS_TERMINATE_JOB_POLL_MS: u64 = 50;
/// Bounded wait for the job to read back empty after `TerminateJobObject`.
const PROCESS_TERMINATE_FORCED_EXIT_WAIT_MS: u64 = 5_000;
/// Exit code given to job members ended by the forced step.
const PROCESS_TERMINATE_FORCED_EXIT_CODE: u32 = 1;

fn process_terminate_refused(
    pid: u32,
    message: impl Into<String>,
    reason: &'static str,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        message.into(),
        Some(json!({
            "code": error_codes::SAFETY_PERMISSION_DENIED,
            "operation": ProcessOperation::Terminate.as_str(),
            "source_of_truth": PROCESS_TERMINATE_SOURCE_OF_TRUTH,
            "source_id": pid.to_string(),
            "reason": reason,
            "remediation": "process terminate only closes a process this MCP session started with process launch; use process history to read the launch pid",
        })),
    )
}

/// Validates `process terminate` input and returns `(pid, grace_ms)`.
pub(super) fn validate_process_terminate_params(
    params: &ProcessParams,
) -> Result<(u32, u64), ErrorData> {
    let operation = ProcessOperation::Terminate;
    let mut unexpected = Vec::new();
    for (field, present) in [
        ("target", params.target.is_some()),
        ("args", params.args.is_some()),
        ("working_dir", params.working_dir.is_some()),
        ("env", params.env.is_some()),
        (
            "wait_for_window_title_regex",
            params.wait_for_window_title_regex.is_some(),
        ),
        ("idempotency_key", params.idempotency_key.is_some()),
        ("cdp_debug", params.cdp_debug.is_some()),
        (
            "force_renderer_accessibility",
            params.force_renderer_accessibility.is_some(),
        ),
        (
            "windows_console_window_state",
            params.windows_console_window_state.is_some(),
        ),
        ("desktop", params.desktop.is_some()),
        (
            "process_name_contains",
            params.process_name_contains.is_some(),
        ),
        (
            "command_line_contains",
            params.command_line_contains.is_some(),
        ),
        ("limit", params.limit.is_some()),
        (
            "include_command_line",
            params.include_command_line.is_some(),
        ),
    ] {
        if present {
            unexpected.push(field);
        }
    }
    process_unexpected_fields(operation, params, &unexpected)?;
    let pid = params.pid.filter(|pid| *pid != 0).ok_or_else(|| {
        process_facade_error(
            operation,
            operation.as_str(),
            "process operation=terminate requires a non-zero pid",
            "pass the pid returned by process launch (or read it from process history)",
        )
    })?;
    let grace_ms = params
        .timeout_ms
        .unwrap_or(PROCESS_TERMINATE_DEFAULT_GRACE_MS);
    if grace_ms == 0 || grace_ms > PROCESS_TERMINATE_MAX_GRACE_MS {
        return Err(process_facade_error(
            operation,
            grace_ms.to_string(),
            format!(
                "process operation=terminate timeout_ms must be between 1 and {PROCESS_TERMINATE_MAX_GRACE_MS}"
            ),
            "use a bounded graceful-exit wait",
        ));
    }
    Ok((pid, grace_ms))
}

/// #333-J: closes a process this MCP session launched. The session
/// process-resource ledger is the ownership authority: a pid this session did
/// not launch through `process launch` is refused before any signal is sent.
/// Every destructive step is bound to the Windows job object created at launch,
/// never to a pid read later, so a pid the OS reused for another process is
/// never closed or killed: the worker pins live job members with open process
/// handles before `WM_CLOSE`, forces with `TerminateJobObject`, and reads the
/// job itself back empty (children whose root already exited included) before
/// it reports `terminated` or releases the ledger. The request's operator-panic
/// epoch is rechecked immediately before every `WM_CLOSE` and before the force.
pub(super) async fn process_terminate_response(
    service: &SynapseService,
    session_id: Option<String>,
    pid: u32,
    grace_ms: u64,
    preflight: &crate::server::action_preflight::ActionPreflightReadback,
) -> Result<ProcessTerminateResponse, ErrorData> {
    let Some(session_id) = session_id else {
        return Err(process_terminate_refused(
            pid,
            "process terminate requires an MCP session: only a process the calling session launched can be closed",
            "no_mcp_session",
        ));
    };
    let Some(owned) = service.session_launched_process(&session_id, pid)? else {
        return Err(process_terminate_refused(
            pid,
            format!(
                "process terminate refused pid {pid}: it was not launched by this MCP session through process launch"
            ),
            "pid_not_launched_by_this_session",
        ));
    };
    let Some(process_job) = owned.process_job else {
        return Err(process_terminate_refused(
            pid,
            format!(
                "process terminate refused pid {pid}: its launch holds no Windows job object, so ownership of a live process cannot be proven"
            ),
            "launch_job_unavailable",
        ));
    };

    let boundary =
        ProcessTerminateMutationBoundary::capture(preflight, "process_terminate_before_worker")?;
    let mut readback = tokio::task::spawn_blocking(move || {
        process_terminate_job(pid, &process_job, grace_ms, &boundary)
    })
    .await
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("process terminate worker failed for pid {pid}: {error}"),
        )
    })?;
    let operator_panic_stop = readback.operator_panic_stop.take();
    let terminated =
        operator_panic_stop.is_none() && readback.job_active_processes_after == Some(0);
    let ledger_released = if terminated {
        service.release_session_launched_process(&session_id, pid)?
    } else {
        false
    };
    tracing::info!(
        code = "PROCESS_TERMINATE_READBACK",
        session_id,
        pid,
        terminated,
        exited_gracefully = readback.exited_gracefully,
        forced = readback.forced_termination.is_some(),
        job_active_after = ?readback.job_active_processes_after,
        remaining = ?readback.remaining_process_ids,
        operator_panic_stop = ?operator_panic_stop.as_ref().map(|(stage, _)| *stage),
        ledger_released,
        "readback=process_terminate"
    );
    let response = ProcessTerminateResponse {
        pid,
        source_of_truth: PROCESS_TERMINATE_SOURCE_OF_TRUTH.to_owned(),
        ownership: "launched_by_this_mcp_session".to_owned(),
        launch_target: owned.launch_target,
        registered_at_unix_ms: owned.registered_at_unix_ms,
        process_ids_before: readback.process_ids_before,
        live_process_ids_before: readback.live_process_ids_before,
        close_requested_hwnds: readback.close_requested_hwnds,
        close_request_errors: readback.close_request_errors,
        graceful_wait_ms: readback.graceful_wait_ms,
        exited_gracefully: readback.exited_gracefully,
        forced_termination: readback.forced_termination,
        remaining_process_ids: readback.remaining_process_ids,
        job_active_processes_after: readback.job_active_processes_after,
        job_readback_errors: readback.job_readback_errors,
        terminated,
        ledger_released,
    };
    if let Some((stage, error)) = operator_panic_stop {
        // Any WM_CLOSE already sent stays in the evidence.
        return Err(super::action_preflight_cleanup_error(
            error,
            stage,
            json!({
                "source_of_truth": PROCESS_TERMINATE_SOURCE_OF_TRUTH,
                "readback": response,
            }),
        ));
    }
    if terminated {
        return Ok(response);
    }
    Err(ErrorData::new(
        ErrorCode(-32099),
        format!(
            "process terminate could not empty the launch job of pid {pid}: job members {:?} (active count {:?}) remain after the graceful close and a forced TerminateJobObject",
            response.remaining_process_ids, response.job_active_processes_after
        ),
        Some(json!({
            "code": error_codes::ACTION_POSTCONDITION_FAILED,
            "operation": ProcessOperation::Terminate.as_str(),
            "source_of_truth": PROCESS_TERMINATE_SOURCE_OF_TRUTH,
            "readback": response,
            "remediation": "read process list for the remaining pids; session teardown also closes this session's launched processes",
        })),
    ))
}

/// Operator-panic admission of one terminate request (#333-J). The routed
/// request reserves its mutation ownership and snapshots its admission epoch
/// before the blocking worker starts; the worker has no task-local boundary,
/// so it rechecks both armed epochs through this value immediately before each
/// physical mutation.
struct ProcessTerminateMutationBoundary {
    preflight: crate::server::action_preflight::ActionPreflightReadback,
    mcp: crate::server::operator_panic_boundary::McpOperatorPanicBoundarySnapshot,
}

impl ProcessTerminateMutationBoundary {
    fn capture(
        preflight: &crate::server::action_preflight::ActionPreflightReadback,
        stage: &'static str,
    ) -> Result<Self, ErrorData> {
        super::ensure_m4_physical_mutation_boundary(preflight, stage)?;
        let mcp =
            crate::server::operator_panic_boundary::reserve_and_snapshot_current_mcp_boundary(
                stage,
            )?;
        Ok(Self {
            preflight: preflight.clone(),
            mcp,
        })
    }

    fn ensure(&self, stage: &'static str) -> Result<(), ErrorData> {
        self.mcp.ensure_admission_epoch(stage)?;
        crate::m2::ensure_operator_panic_action_admission(
            self.preflight.tool,
            stage,
            self.preflight.operator_panic_epoch_at_arm,
        )
    }
}

#[derive(Default)]
struct ProcessTerminateJobReadback {
    process_ids_before: Vec<u32>,
    live_process_ids_before: Vec<u32>,
    close_requested_hwnds: Vec<i64>,
    close_request_errors: Vec<String>,
    graceful_wait_ms: u64,
    exited_gracefully: bool,
    forced_termination: Option<crate::m4::OwnedProcessTerminationReadback>,
    remaining_process_ids: Vec<u32>,
    job_active_processes_after: Option<u32>,
    job_readback_errors: Vec<String>,
    /// The mutation boundary that refused after the operator panic, with its error.
    operator_panic_stop: Option<(&'static str, ErrorData)>,
}

/// Blocking graceful-then-forced close of one launch job. Nothing here
/// resolves a pid into a process to act on: windows are matched only to
/// pinned job members, and the force ends the job itself.
fn process_terminate_job(
    pid: u32,
    job: &crate::m4::OwnedProcessJob,
    grace_ms: u64,
    boundary: &ProcessTerminateMutationBoundary,
) -> ProcessTerminateJobReadback {
    let mut readback = ProcessTerminateJobReadback::default();
    match job.member_process_ids() {
        Ok(process_ids) => readback.process_ids_before = process_ids,
        Err(error) => readback
            .job_readback_errors
            .push(format!("job membership before close: {error}")),
    }
    let members = match job.pin_live_members(&readback.process_ids_before) {
        Ok((members, unproven)) => {
            readback.close_request_errors.extend(unproven);
            members
        }
        Err(error) => {
            readback
                .job_readback_errors
                .push(format!("job member pinning before close: {error}"));
            Vec::new()
        }
    };
    readback.live_process_ids_before = members
        .iter()
        .map(crate::m4::OwnedJobMemberProcess::pid)
        .collect();
    process_terminate_request_close(&members, boundary, &mut readback);
    // The pins are needed only while windows are matched to members.
    drop(members);

    if readback.operator_panic_stop.is_none() && !readback.close_requested_hwnds.is_empty() {
        let (active, waited_ms) = wait_for_owned_job_empty(job, Duration::from_millis(grace_ms));
        readback.graceful_wait_ms = waited_ms;
        readback.exited_gracefully = matches!(active, Ok(0));
        if let Err(error) = active {
            readback
                .job_readback_errors
                .push(format!("job active count during graceful wait: {error}"));
        }
    }
    if readback.operator_panic_stop.is_none()
        && !readback.exited_gracefully
        && !matches!(job.active_process_count(), Ok(0))
    {
        let stage = "process_terminate_before_force";
        match boundary.ensure(stage) {
            Ok(()) => readback.forced_termination = Some(process_terminate_force_job(pid, job)),
            Err(error) => readback.operator_panic_stop = Some((stage, error)),
        }
    }
    process_terminate_final_job_readback(job, &mut readback);
    readback
}

/// Sends `WM_CLOSE` to the visible top-level windows of pinned job members,
/// rechecking the operator-panic epoch immediately before each one.
fn process_terminate_request_close(
    members: &[crate::m4::OwnedJobMemberProcess],
    boundary: &ProcessTerminateMutationBoundary,
    readback: &mut ProcessTerminateJobReadback,
) {
    if members.is_empty() {
        return;
    }
    let windows = match synapse_a11y::visible_top_level_window_contexts() {
        Ok(windows) => windows,
        Err(error) => {
            readback
                .close_request_errors
                .push(format!("visible window enumeration failed: {error}"));
            return;
        }
    };
    for window in &windows {
        let Some(member) = members.iter().find(|member| member.pid() == window.pid) else {
            continue;
        };
        let stage = "process_terminate_before_wm_close";
        if let Err(error) = boundary.ensure(stage) {
            readback.operator_panic_stop = Some((stage, error));
            return;
        }
        match member.post_close_to_own_window(window.hwnd) {
            Ok(true) => readback.close_requested_hwnds.push(window.hwnd),
            Ok(false) => {}
            Err(error) => readback.close_request_errors.push(error),
        }
    }
}

/// Ends every process in the launch job with `TerminateJobObject` and waits
/// for the job to read back empty.
fn process_terminate_force_job(
    pid: u32,
    job: &crate::m4::OwnedProcessJob,
) -> crate::m4::OwnedProcessTerminationReadback {
    let process_ids = job.member_process_ids().unwrap_or_default();
    let terminate = job.terminate_all(PROCESS_TERMINATE_FORCED_EXIT_CODE);
    let active = if terminate.is_ok() {
        wait_for_owned_job_empty(
            job,
            Duration::from_millis(PROCESS_TERMINATE_FORCED_EXIT_WAIT_MS),
        )
        .0
    } else {
        job.active_process_count()
    };
    let status = match (&terminate, &active) {
        (Err(error), _) => format!("terminate_job_object_failed:{error}"),
        (Ok(()), Ok(0)) => "terminate_job_object_job_empty".to_owned(),
        (Ok(()), Ok(count)) => format!("terminate_job_object_job_not_empty:active={count}"),
        (Ok(()), Err(error)) => format!("terminate_job_object_readback_failed:{error}"),
    };
    let remaining_process_ids = if matches!(active, Ok(0)) {
        Vec::new()
    } else {
        job.member_process_ids()
            .unwrap_or_else(|_| process_ids.clone())
    };
    crate::m4::OwnedProcessTerminationReadback {
        pid,
        process_ids: process_ids.clone(),
        live_process_ids_before: process_ids,
        attempted: true,
        status,
        remaining_process_ids,
    }
}

/// The exit readback: the job's own live process count and member list.
fn process_terminate_final_job_readback(
    job: &crate::m4::OwnedProcessJob,
    readback: &mut ProcessTerminateJobReadback,
) {
    match job.active_process_count() {
        Ok(0) => {
            readback.job_active_processes_after = Some(0);
            readback.remaining_process_ids = Vec::new();
        }
        Ok(count) => {
            readback.job_active_processes_after = Some(count);
            match job.member_process_ids() {
                Ok(process_ids) => readback.remaining_process_ids = process_ids,
                Err(error) => readback
                    .job_readback_errors
                    .push(format!("job membership after close: {error}")),
            }
        }
        Err(error) => readback
            .job_readback_errors
            .push(format!("job active count after close: {error}")),
    }
}

/// Polls the job's live process count until it reads zero, the readback
/// fails, or `timeout` elapses. Returns the last readback and the waited ms.
fn wait_for_owned_job_empty(
    job: &crate::m4::OwnedProcessJob,
    timeout: Duration,
) -> (Result<u32, String>, u64) {
    let started = std::time::Instant::now();
    loop {
        let active = job.active_process_count();
        if matches!(active, Ok(0) | Err(_)) || started.elapsed() >= timeout {
            let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            return (active, waited_ms);
        }
        std::thread::sleep(Duration::from_millis(PROCESS_TERMINATE_JOB_POLL_MS));
    }
}

pub(super) fn json_u32_field(value: &Value, field: &str) -> Option<u32> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
}

pub(super) fn json_string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}
