#[path = "shell_facade.rs"]
mod shell_facade;
#[path = "shell_gui_launch.rs"]
mod shell_gui_launch;

use super::{
    ActComboParams, ActComboResponse, ActLaunchParams, ActLaunchResponse,
    ActRunShellCancelResponse, ActRunShellJobIdParams, ActRunShellParams, ActRunShellResponse,
    ActRunShellStartParams, ActRunShellStartResponse, ActRunShellStatusParams,
    ActRunShellStatusResponse, ActSpawnAgentCli, ActSpawnAgentLogPaths, ActSpawnAgentParams,
    ActSpawnAgentRequest, ActSpawnAgentResponse, ActSpawnAgentTarget, AgentSpawnTaskStartedParams,
    AgentSpawnTaskStartedResponse, ErrorData, Json, LaunchWindowState,
    MAX_AGENT_SPAWN_WAIT_TIMEOUT_MS, Parameters, RUN_SHELL_IDEMPOTENCY_LEGACY_SCAN_LIMIT,
    RUN_SHELL_IDEMPOTENCY_PREFIX, RunShellAuthorization, SessionTarget, ShellExecutionContext,
    SynapseService, TargetWire, assign_owned_process_job, authorize_run_shell,
    authorize_run_shell_start, cancel_shell_job, execute_combo, launch_for_session,
    launch_process_history_row, launch_process_history_row_key, launch_request_details, mcp_error,
    prepare_run_shell_params_for_context, prepare_run_shell_start_params_for_context,
    required_combo_permissions, run_authorized_shell, run_shell_idempotency_completed_row,
    run_shell_idempotency_recover_after_restart, run_shell_idempotency_replay,
    run_shell_idempotency_reservation_row, run_shell_idempotency_row_key,
    run_shell_idempotency_select_legacy_row, run_shell_request_details,
    run_shell_start_request_details, shell_execution_context_for_session, shell_job_status,
    start_authorized_shell_job, tool, tool_router, validate_agent_spawn_params,
    validate_run_shell_execution_plan,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

use rmcp::{RoleServer, model::ErrorCode, schemars::JsonSchema, service::RequestContext};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use synapse_core::{error_codes, new_reflex_id};
use synapse_storage::{cf, decode_json};

use crate::m3::local_models::{
    LocalModelApiShape, LocalModelProbeParams, LocalModelRegistryRow, ResolvedApiKey,
};
use crate::m4::{ActRunShellExecutionMode, redact_log_window};

use super::{
    m1_tools::validate_target_window,
    session_registry::{
        SessionRegistryRead, SpawnedAgentControlRead, SpawnedAgentRead, unix_time_ms_now,
    },
    terminal_capture::capture::{
        CaptureArtifacts, CaptureSpec, spawn_capture_to_asciicast_with_job,
    },
};

static RUN_SHELL_RESERVATION_OWNER_ID: LazyLock<String> =
    LazyLock::new(|| uuid::Uuid::new_v4().to_string());
static LIVE_RUN_SHELL_RESERVATIONS: LazyLock<Mutex<BTreeSet<Vec<u8>>>> =
    LazyLock::new(|| Mutex::new(BTreeSet::new()));

pub(crate) struct LiveRunShellReservation {
    row_key: Vec<u8>,
}

impl LiveRunShellReservation {
    pub(crate) fn register(row_key: Vec<u8>) -> Result<Self, ErrorData> {
        let mut live = LIVE_RUN_SHELL_RESERVATIONS.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "live shell idempotency registry lock poisoned",
            )
        })?;
        if !live.insert(row_key.clone()) {
            return Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "shell idempotency reservation unexpectedly already live",
            ));
        }
        Ok(Self { row_key })
    }

    pub(crate) fn is_live(row_key: &[u8]) -> Result<bool, ErrorData> {
        LIVE_RUN_SHELL_RESERVATIONS
            .lock()
            .map(|live| live.contains(row_key))
            .map_err(|_error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "live shell idempotency registry lock poisoned",
                )
            })
    }
}

impl Drop for LiveRunShellReservation {
    fn drop(&mut self) {
        if let Ok(mut live) = LIVE_RUN_SHELL_RESERVATIONS.lock() {
            live.remove(&self.row_key);
        }
    }
}

const ACT_SPAWN_AGENT: &str = "act_spawn_agent";
/// Filename of the per-spawn manifest written into each spawn dir. The
/// transcript ingester (#900/#949) reads it to learn the spawn's model when the
/// CLI stream does not carry one (Codex).
pub(crate) const AGENT_SPAWN_MANIFEST_FILENAME: &str = "spawn-manifest.json";
/// Schema version stamped onto the spawn manifest.
pub(crate) const AGENT_SPAWN_MANIFEST_VERSION: u32 = 1;
const CODEX_APP_SERVER_RUNNER_SCRIPT: &str = include_str!("codex_app_server_runner.ps1");
const SHELL_FACADE_SOURCE_OF_TRUTH: &str = "%LOCALAPPDATA%\\Synapse\\shell-jobs + %LOCALAPPDATA%\\Synapse\\shell-sessions + daemon-tool-events.jsonl";
const PROCESS_FACADE_SOURCE_OF_TRUTH: &str = "live OS process table + CF_PROCESS_HISTORY";
const PROCESS_LIST_DEFAULT_LIMIT: usize = 100;
const PROCESS_LIST_MAX_LIMIT: usize = 1000;
const PROCESS_HISTORY_DEFAULT_LIMIT: usize = 20;
const PROCESS_HISTORY_MAX_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShellOperation {
    #[default]
    Run,
    Start,
    Status,
    Cancel,
}

impl ShellOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Start => "start",
            Self::Status => "status",
            Self::Cancel => "cancel",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellParams {
    #[serde(default)]
    #[schemars(description = "Shell facade operation. Defaults to run.")]
    pub operation: ShellOperation,
    #[serde(default)]
    #[schemars(description = "Executable path/name only for run/start operations.")]
    pub command: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "Literal executable arguments for run/start.")]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    #[schemars(default)]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub execution_mode: Option<ActRunShellExecutionMode>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub durable_timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    #[schemars(length(min = 1, max = 128))]
    pub job_id: Option<String>,
    #[serde(default)]
    #[schemars(range(min = 0, max = 1048576))]
    pub tail_bytes: Option<u64>,
    /// run/start only: explicit per-call owner approval, audited with the reason.
    #[serde(default)]
    pub owner_approved: Option<bool>,
    /// Required non-empty when owner_approved=true; owner_approval_reason is an alias.
    #[serde(default, alias = "owner_approval_reason")]
    pub reason: Option<String>,
}

fn shell_input_schema() -> Arc<Map<String, Value>> {
    shell_facade::input_schema()
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellFacadeResponse {
    pub operation: ShellOperation,
    pub source_of_truth: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<ActRunShellResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<ActRunShellStartResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ActRunShellStatusResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<ActRunShellCancelResponse>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessOperation {
    #[default]
    List,
    Launch,
    History,
}

impl ProcessOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Launch => "launch",
            Self::History => "history",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessParams {
    #[serde(default)]
    #[schemars(description = "Process facade operation. Defaults to list.")]
    pub operation: ProcessOperation,
    #[serde(default)]
    #[schemars(description = "Executable path/name for launch operations.")]
    pub target: Option<String>,
    #[serde(default)]
    #[schemars(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    #[schemars(default)]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub wait_for_window_title_regex: Option<String>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub cdp_debug: Option<bool>,
    #[serde(default)]
    pub force_renderer_accessibility: Option<bool>,
    #[serde(default)]
    pub windows_console_window_state: Option<LaunchWindowState>,
    #[serde(default)]
    pub desktop: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub process_name_contains: Option<String>,
    #[serde(default)]
    pub command_line_contains: Option<String>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 1000))]
    pub limit: Option<usize>,
    #[serde(default)]
    #[schemars(default)]
    pub include_command_line: Option<bool>,
    /// #258, operation=launch only: `true` with a non-empty `reason` records
    /// that the owner explicitly asked for this GUI launch while using the PC.
    /// It bypasses the USER_ACTIVE / USER_ACTIVITY_UNKNOWN user-activity gate
    /// for this call only and is audited with the reason.
    #[serde(default)]
    pub owner_approved: Option<bool>,
    /// #258, operation=launch only: the owner's reason; required (non-empty)
    /// when owner_approved=true.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessFacadeResponse {
    pub operation: ProcessOperation,
    pub source_of_truth: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<ActLaunchResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processes: Option<ProcessListResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<ProcessHistoryResponse>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessListResponse {
    pub source_of_truth: String,
    pub returned_count: usize,
    pub limit: usize,
    pub filters: ProcessFilters,
    pub rows: Vec<ProcessRow>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_name_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_line_contains: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessRow {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pid: Option<u32>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub status: String,
    pub start_time_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_line: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessHistoryResponse {
    pub source_of_truth: String,
    pub cf_name: String,
    pub returned_count: usize,
    pub scanned_tail_rows: usize,
    pub limit: usize,
    pub filters: ProcessFilters,
    pub rows: Vec<ProcessHistoryRow>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessHistoryRow {
    pub key: String,
    pub key_hex: String,
    pub value_len_bytes: u64,
    pub row_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_line: Option<String>,
}

/// Builds the per-spawn manifest JSON. Records the CLI and, when the operator
/// pinned one, the model — the authoritative model source the transcript
/// ingester reads (indispensable for Codex, whose stream omits it, #949) — and
/// the resolved working directory plus the runtime settings (mcp_url, approval
/// gate, wait/hold budgets, target), which respawn reuses unchanged (#308).
fn build_spawn_manifest(
    spawn_id: &str,
    params: &ActSpawnAgentParams,
    working_dir: &Path,
) -> Result<Value, ErrorData> {
    let agent_kind = params.effective_cli()?;
    Ok(json!({
        "version": AGENT_SPAWN_MANIFEST_VERSION,
        "spawn_id": spawn_id,
        "cli": agent_kind.as_str(),
        "kind": agent_kind.as_str(),
        "model": params.model_for_spawn_manifest(agent_kind),
        "model_ref": params.local_model_ref(),
        "working_dir": working_dir.display().to_string(),
        "mcp_url": params.mcp_url,
        "wait_timeout_ms": params.wait_timeout_ms,
        "hold_open_ms": params.hold_open_ms,
        "target": params.target,
        "require_approval_gate": params.require_approval_gate,
        "approval_gate_effective": params.require_approval_gate && agent_kind.uses_approval_gate(),
        "local_model_autonomous_tool_calls": agent_kind.is_local_model(),
        "local_model_approval_gate_used": false,
        "local_model_trusted_unattended_exact_contract": agent_kind.is_local_model(),
        "assigned_prompt_present": params
            .prompt
            .as_deref()
            .is_some_and(|prompt| !prompt.trim().is_empty()),
        // Spawn-template provenance (#909): the exact template version + config
        // hash this spawn was rendered from, or null for a direct spawn. The
        // manifest is the physical source of truth for run reproducibility.
        "template_id": params.template_id.as_deref(),
        "template_version": params.template_version,
        "template_config_hash": params.template_config_hash.as_deref(),
        "created_unix_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0),
    }))
}
const AGENT_SPAWN_SHELL_ENV_VAR: &str = "SYNAPSE_AGENT_SPAWN_SHELL";
const AGENT_SPAWN_RECORDED_ATTEMPT_LIMIT: usize = 80;
const AGENT_SPAWN_POLL_INTERVAL_MS: u64 = 250;
const AGENT_SPAWN_LOG_TAIL_BYTES: usize = 8 * 1024;
const AGENT_SPAWN_ORPHAN_RECOVERY_STALE_MS: u64 = 10 * 60 * 1000;
const LOCAL_MODEL_SPAWN_MAX_PROBE_AGE_MS: u64 = 15 * 60 * 1000;
static AGENT_SPAWN_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static AGENT_SPAWN_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct AgentSpawnInFlightGuard {
    spawn_id: String,
    sequence: u64,
    in_flight_at_start: u64,
    started_at: Instant,
}

impl AgentSpawnInFlightGuard {
    fn enter(spawn_id: &str, cli: ActSpawnAgentCli) -> Self {
        let sequence = AGENT_SPAWN_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let in_flight_at_start = AGENT_SPAWN_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::info!(
            code = "AGENT_SPAWN_IN_FLIGHT_ENTER",
            spawn_id,
            cli = cli.as_str(),
            sequence,
            in_flight_at_start,
            "act_spawn_agent entered provisioning"
        );
        Self {
            spawn_id: spawn_id.to_owned(),
            sequence,
            in_flight_at_start,
            started_at: Instant::now(),
        }
    }

    fn in_flight_now() -> u64 {
        AGENT_SPAWN_IN_FLIGHT.load(Ordering::SeqCst)
    }
}

impl Drop for AgentSpawnInFlightGuard {
    fn drop(&mut self) {
        let before = AGENT_SPAWN_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        let in_flight_after = before.saturating_sub(1);
        tracing::info!(
            code = "AGENT_SPAWN_IN_FLIGHT_EXIT",
            spawn_id = %self.spawn_id,
            sequence = self.sequence,
            in_flight_after,
            elapsed_ms = duration_ms_u64(self.started_at.elapsed()),
            "act_spawn_agent left provisioning"
        );
    }
}

#[derive(Debug)]
struct AgentSpawnTiming {
    request_started: Instant,
    prelaunch_done: Option<Instant>,
    launch_started: Option<Instant>,
    launch_completed: Option<Instant>,
    session_wait_started: Option<Instant>,
    session_matched: Option<Instant>,
    task_wait_started: Option<Instant>,
    task_started: Option<Instant>,
}

impl AgentSpawnTiming {
    fn new() -> Self {
        Self {
            request_started: Instant::now(),
            prelaunch_done: None,
            launch_started: None,
            launch_completed: None,
            session_wait_started: None,
            session_matched: None,
            task_wait_started: None,
            task_started: None,
        }
    }

    fn mark_prelaunch_done(&mut self) {
        self.prelaunch_done = Some(Instant::now());
    }

    fn mark_launch_started(&mut self) {
        self.launch_started = Some(Instant::now());
    }

    fn mark_launch_completed(&mut self) {
        self.launch_completed = Some(Instant::now());
    }

    fn mark_session_wait_started(&mut self) {
        self.session_wait_started = Some(Instant::now());
    }

    fn mark_session_matched(&mut self) {
        self.session_matched = Some(Instant::now());
    }

    fn mark_task_wait_started(&mut self) {
        self.task_wait_started = Some(Instant::now());
    }

    fn mark_task_started(&mut self) {
        self.task_started = Some(Instant::now());
    }

    fn readback(&self, guard: &AgentSpawnInFlightGuard, wait_timeout_ms: u64) -> Value {
        let now = Instant::now();
        json!({
            "source_of_truth": "act_spawn_agent in-process timing/in-flight counters plus per-spawn artifacts",
            "spawn_sequence": guard.sequence,
            "in_flight_at_start": guard.in_flight_at_start,
            "in_flight_now": AgentSpawnInFlightGuard::in_flight_now(),
            "wait_timeout_ms": wait_timeout_ms,
            "queue_wait_ms": 0,
            "queue_wait_source": "no_explicit_spawn_queue; readiness deadline starts after this spawn launches",
            "request_elapsed_ms": elapsed_ms_between(self.request_started, now),
            "prelaunch_elapsed_ms": self.prelaunch_done.map(|end| elapsed_ms_between(self.request_started, end)),
            "launch_elapsed_ms": match (self.launch_started, self.launch_completed) {
                (Some(start), Some(end)) => Some(elapsed_ms_between(start, end)),
                (Some(start), None) => Some(elapsed_ms_between(start, now)),
                _ => None,
            },
            "session_wait_elapsed_ms": match (self.session_wait_started, self.session_matched) {
                (Some(start), Some(end)) => Some(elapsed_ms_between(start, end)),
                (Some(start), None) => Some(elapsed_ms_between(start, now)),
                _ => None,
            },
            "task_start_wait_elapsed_ms": match (self.task_wait_started, self.task_started) {
                (Some(start), Some(end)) => Some(elapsed_ms_between(start, end)),
                (Some(start), None) => Some(elapsed_ms_between(start, now)),
                _ => None,
            },
            "deadline_policy": "per_spawn_phase_deadlines_started_after_launch_and_after_session_match",
        })
    }
}

fn duration_ms_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn elapsed_ms_between(start: Instant, end: Instant) -> u64 {
    duration_ms_u64(end.saturating_duration_since(start))
}

fn shell_facade_error(
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

fn process_facade_error(
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

fn shell_facade_delegate_error(
    operation: ShellOperation,
    source_id: impl Into<String>,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    let source_id = source_id.into();
    let cause_code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned();
    let cause_data = error.data.clone().unwrap_or(Value::Null);
    ErrorData::new(
        error.code,
        error.message.to_string(),
        Some(json!({
            "code": cause_code,
            "operation": operation.as_str(),
            "source_of_truth": SHELL_FACADE_SOURCE_OF_TRUTH,
            "source_id": source_id,
            "remediation": remediation,
            "cause": cause_data,
        })),
    )
}

fn process_facade_delegate_error(
    operation: ProcessOperation,
    source_id: impl Into<String>,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    let source_id = source_id.into();
    let cause_code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned();
    let cause_data = error.data.clone().unwrap_or(Value::Null);
    ErrorData::new(
        error.code,
        error.message.to_string(),
        Some(json!({
            "code": cause_code,
            "operation": operation.as_str(),
            "source_of_truth": PROCESS_FACADE_SOURCE_OF_TRUTH,
            "source_id": source_id,
            "remediation": remediation,
            "cause": cause_data,
        })),
    )
}

fn require_shell_text(
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

fn require_process_text(
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

fn shell_unexpected_fields(
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

fn process_unexpected_fields(
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

fn shell_run_params(params: ShellParams) -> Result<ActRunShellParams, ErrorData> {
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
        original_working_dir: None,
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

fn shell_start_params(params: ShellParams) -> Result<ActRunShellStartParams, ErrorData> {
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

fn shell_status_params(params: ShellParams) -> Result<ActRunShellStatusParams, ErrorData> {
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

fn shell_cancel_params(params: ShellParams) -> Result<ActRunShellJobIdParams, ErrorData> {
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

fn process_launch_params(params: ProcessParams) -> Result<ActLaunchParams, ErrorData> {
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

fn validate_process_query_params(
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
    if params.owner_approved.is_some() {
        unexpected.push("owner_approved");
    }
    if params.reason.is_some() {
        unexpected.push("reason");
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
        ProcessOperation::Launch => unreachable!("launch is not a query operation"),
    });
    let max_limit = match operation {
        ProcessOperation::List => PROCESS_LIST_MAX_LIMIT,
        ProcessOperation::History => PROCESS_HISTORY_MAX_LIMIT,
        ProcessOperation::Launch => unreachable!("launch is not a query operation"),
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

fn process_filters(params: &ProcessParams) -> ProcessFilters {
    ProcessFilters {
        pid: params.pid,
        process_name_contains: params.process_name_contains.clone(),
        command_line_contains: params.command_line_contains.clone(),
    }
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn process_row_matches(filters: &ProcessFilters, row: &ProcessRow) -> bool {
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

fn process_history_row_matches(filters: &ProcessFilters, row: &ProcessHistoryRow) -> bool {
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

fn process_list_response(params: &ProcessParams) -> Result<ProcessListResponse, ErrorData> {
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

fn process_history_response(
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
                hex_lower(&key),
                format!("CF_PROCESS_HISTORY row decode failed: {error}"),
                "inspect the exact process history row bytes and fix the writer",
            )
        })?;
        let row_json = serde_json::to_string(&decoded).map_err(|error| {
            process_facade_error(
                ProcessOperation::History,
                hex_lower(&key),
                format!("CF_PROCESS_HISTORY row JSON render failed: {error}"),
                "inspect the decoded process history row",
            )
        })?;
        let row = ProcessHistoryRow {
            key: String::from_utf8_lossy(&key).into_owned(),
            key_hex: hex_lower(&key),
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

fn json_u32_field(value: &Value, field: &str) -> Option<u32> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
}

fn json_string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(windows)]
const AGENT_SPAWN_WINDOWS_SHELL_CANDIDATES: &[(&str, &str)] = &[
    ("path:pwsh.exe", "pwsh.exe"),
    (
        "known_path:powershell7_x64",
        r"C:\Program Files\PowerShell\7\pwsh.exe",
    ),
    (
        "known_path:powershell7_x86",
        r"C:\Program Files (x86)\PowerShell\7\pwsh.exe",
    ),
    ("path:powershell.exe", "powershell.exe"),
    (
        "known_path:windows_powershell",
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
    ),
];

#[tool_router(router = m4_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Execute a timed one-shot sequence of act_press key steps only; use act_stroke for continuous mouse motion/path execution"
    )]
    pub async fn act_combo(
        &self,
        params: Parameters<ActComboParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActComboResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_combo",
            step_count = params.0.steps.len(),
            "tool.invocation kind=act_combo"
        );
        let required = required_combo_permissions(&params.0)?;
        self.require_m3_permissions("act_combo", &required)?;
        // #258: combo key steps reach the human's real keyboard focus.
        let session_id = super::context::mcp_session_id_from_request_context(&request_context)?;
        if let Err(error) = self
            .ensure_supported_use_allows_action("act_combo")
            .and_then(|_| self.ensure_user_idle("act_combo", session_id.as_deref()))
        {
            self.audit_action_denied_for_request("act_combo", &error, &request_context);
            return Err(error);
        }
        self.refresh_reflex_audit_context()?;
        // #198: fallible lookups run before the started row, so a failure can
        // never leave an orphan "started" row in CF_ACTION_LOG.
        let runtime = self.reflex_runtime()?;
        self.install_reflex_action_gate(&runtime)?;
        let (handle, _, _) = self.m2_action_context_for_session_id(session_id.clone())?;
        let input_target = handle
            .delivery_target()
            .map_err(|error| crate::m2::action_error_to_mcp(&error))?
            .map(|target| target.stored());
        self.audit_action_started_for_request("act_combo", &request_context)?;
        let result = execute_combo(runtime, params.0, input_target, session_id).await;
        self.audit_action_result_for_request("act_combo", &result, &request_context)?;
        result.map(Json)
    }

    #[tool(
        description = "Facade for shell execution. operation=run executes an allowlisted child process; start creates a durable shell job with artifact paths and uses timeout_ms as the durable lifetime cap; status reads persisted job/output state; cancel terminates the exact durable job and returns before/after readback. durable_timeout_ms, execution_mode, and idempotency_key are run-only fields and are invalid for start/status/cancel. run/start refuse visible-desktop GUI launch verbs and resolved GUI-subsystem executables before spawn while the owner is active or activity is unknown (#258). Wait for the quiet window, use process operation=launch with approval, or pass owner_approved=true plus a non-empty reason (alias owner_approval_reason) for this call; approval is audited. Console commands and idle routes are unchanged. Detection does not inspect GUI launches inside script files.",
        input_schema = shell_input_schema()
    )]
    pub async fn shell(
        &self,
        params: Parameters<ShellParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ShellFacadeResponse>, ErrorData> {
        let params = params.0;
        let operation = params.operation;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "shell",
            operation = operation.as_str(),
            "tool.invocation kind=shell"
        );
        let approval = shell_facade::owner_approval(&params, &request_context)?;
        super::owner_override::ApprovedCall::scope(
            approval,
            self.dispatch_shell(params, request_context),
        )
        .await
    }

    #[tool(
        description = "Facade for process capability. operation=list reads the live OS process table; launch delegates to the audited process launcher and records CF_PROCESS_HISTORY; history reads decoded CF_PROCESS_HISTORY rows for launch readback."
    )]
    pub async fn process(
        &self,
        params: Parameters<ProcessParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ProcessFacadeResponse>, ErrorData> {
        let params = params.0;
        let operation = params.operation;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "process",
            operation = operation.as_str(),
            "tool.invocation kind=process"
        );
        match operation {
            ProcessOperation::List => {
                let response = process_list_response(&params)?;
                Ok(Json(ProcessFacadeResponse {
                    operation,
                    source_of_truth: PROCESS_FACADE_SOURCE_OF_TRUTH.to_owned(),
                    launch: None,
                    processes: Some(response),
                    history: None,
                }))
            }
            ProcessOperation::Launch => {
                // #258: a per-call owner override of the user-activity gate.
                let owner_override = super::owner_override::OwnerOverride::from_request(
                    "process operation=launch",
                    params.owner_approved,
                    params.reason.as_deref(),
                )?;
                let low_params = process_launch_params(params)?;
                let source_id = low_params.target.clone();
                let response = self
                    .act_launch_with_override(low_params, owner_override, request_context)
                    .await
                    .map_err(|error| {
                        process_facade_delegate_error(
                            operation,
                            source_id,
                            error,
                            "fix the target executable/path or launch policy, then verify CF_PROCESS_HISTORY and the process table",
                        )
                    })?
                    .0;
                Ok(Json(ProcessFacadeResponse {
                    operation,
                    source_of_truth: PROCESS_FACADE_SOURCE_OF_TRUTH.to_owned(),
                    launch: Some(response),
                    processes: None,
                    history: None,
                }))
            }
            ProcessOperation::History => {
                let response = process_history_response(self, &params)?;
                Ok(Json(ProcessFacadeResponse {
                    operation,
                    source_of_truth: PROCESS_FACADE_SOURCE_OF_TRUTH.to_owned(),
                    launch: None,
                    processes: None,
                    history: Some(response),
                }))
            }
        }
    }

    #[tool(
        description = "Run an allowlisted executable child process. command is an executable path/name only; pass flags and shell snippets in args, using an explicit shell executable when shell syntax is required. Do not use this to launch headed Playwright/Chromium automation: Synapse refuses direct or shell-wrapped Chrome/Playwright remote-debugging launches that can surface Chrome debugger/automation banners and shift browser layout. Use the existing authenticated Chrome through cdp_* / target_act / browser_* tools, or act_launch with Synapse-injected isolated CDP flags. execution_mode controls routing: auto preserves compatibility and backgrounds when timeout_ms exceeds the inline await limit, inline waits only while timeout_ms fits inside the MCP client-call budget and otherwise returns a durable job handle, durable returns a job handle immediately. durable_timeout_ms is an explicit durable job lifetime cap only when a durable/background job is created; it is ignored when execution completes inline. Omit it for an unbounded durable job. Poll act_run_shell_status and cancel with act_run_shell_cancel."
    )]
    pub async fn act_run_shell(
        &self,
        params: Parameters<ActRunShellParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActRunShellResponse>, ErrorData> {
        let mut raw_params = params.0;
        let shell_context = self.prepare_shell_context(
            "act_run_shell",
            &mut raw_params.command,
            &raw_params.args,
            &raw_params.env,
            raw_params.working_dir.as_deref(),
            &request_context,
        )?;
        let params = prepare_run_shell_params_for_context(raw_params, &shell_context)?;
        let launch_guard = self.guard_shell_gui_launch(
            &params.command,
            &params.args,
            params.working_dir.as_deref(),
            &params.env,
            shell_context.session_id(),
        );
        self.audit_shell_launch_guard("act_run_shell", launch_guard, &request_context)?;
        let audit = self.begin_shell_run_audit(&params, &shell_context)?;
        let result = match authorize_run_shell(&self.m4_config, &params) {
            Ok(authorization) => {
                run_shell_with_idempotency(
                    self,
                    params,
                    authorization,
                    self.m4_config.run_shell_inline_await_limit_ms(),
                    Some(&shell_context),
                )
                .await
            }
            Err(error) => Err(error),
        };
        self.finish_shell_audit(audit, &result)?;
        result.map(Json)
    }

    #[tool(
        description = "Start an allowlisted executable as a durable background shell job. Do not use this to launch headed Playwright/Chromium automation: Synapse refuses direct or shell-wrapped Chrome/Playwright remote-debugging launches that can surface Chrome debugger/automation banners and shift browser layout. Use existing authenticated Chrome via cdp_* / target_act / browser_* tools, or act_launch with Synapse-injected isolated CDP flags. Returns immediately with a job id plus status/stdout/stderr file paths. Omitting timeout_ms leaves the durable job unbounded until normal exit, explicit act_run_shell_cancel, or session cleanup; providing timeout_ms is an explicit lifetime cap for that job only."
    )]
    pub async fn act_run_shell_start(
        &self,
        params: Parameters<ActRunShellStartParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActRunShellStartResponse>, ErrorData> {
        let mut raw_params = params.0;
        let shell_context = self.prepare_shell_context(
            "act_run_shell_start",
            &mut raw_params.command,
            &raw_params.args,
            &raw_params.env,
            raw_params.working_dir.as_deref(),
            &request_context,
        )?;
        let params = prepare_run_shell_start_params_for_context(raw_params, &shell_context)?;
        let launch_guard = self.guard_shell_gui_launch(
            &params.command,
            &params.args,
            params.working_dir.as_deref(),
            &params.env,
            shell_context.session_id(),
        );
        self.audit_shell_launch_guard("act_run_shell_start", launch_guard, &request_context)?;
        let audit = self.begin_shell_start_audit(&params, &shell_context)?;
        let result = match authorize_run_shell_start(&self.m4_config, &params) {
            Ok(authorization) => {
                start_authorized_shell_job(params, &authorization, Some(&shell_context))
            }
            Err(error) => Err(error),
        };
        self.finish_shell_audit(audit, &result)?;
        result.map(Json)
    }

    #[tool(
        description = "Read durable background shell job status, no-output/stall diagnostics, process-tree summary, SSH/SCP/SFTP transfer hints, and tails of persisted stdout/stderr logs by job id. This is a separate source-of-truth readback and does not wait for the process to finish."
    )]
    pub async fn act_run_shell_status(
        &self,
        params: Parameters<ActRunShellStatusParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActRunShellStatusResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_run_shell_status",
            job_id = %params.0.job_id,
            "tool.invocation kind=act_run_shell_status"
        );
        if let Err(error) = self.ensure_supported_use_allows_action("act_run_shell") {
            self.audit_action_denied_for_request("act_run_shell_status", &error, &request_context);
            return Err(error);
        }
        let params = params.0;
        let session_id = require_shell_session_id(&request_context)?;
        self.audit_action_started_with_details_for_session(
            "act_run_shell_status",
            &json!({
                "job_id": &params.job_id,
                "tail_bytes": params.tail_bytes,
                "session_id": &session_id,
            }),
            &session_id,
        )?;
        // Status reads files and the process table: keep it off the async
        // runtime so a slow read never parks a worker (upstream 21341566).
        let worker_session_id = session_id.clone();
        let result = crate::m4::spawn_shell_job_blocking(move || {
            shell_job_status(&params, Some(&worker_session_id))
        })
        .await
        .unwrap_or_else(|error| {
            Err(mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("act_run_shell_status worker failed: {error}"),
            ))
        });
        self.audit_action_result_for_session("act_run_shell_status", &result, &session_id)?;
        result.map(Json)
    }

    #[tool(
        description = "Cancel a durable background shell job by exact job id, terminating the recorded local job-owned process tree and returning status/log/process readback. Direct SSH jobs with a tracked remote pid/process-group attempt bounded remote cleanup and report whether that cleanup was verified; untracked SSH modes fail closed with the remote cleanup handle/status marked unverified."
    )]
    pub async fn act_run_shell_cancel(
        &self,
        params: Parameters<ActRunShellJobIdParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActRunShellCancelResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_run_shell_cancel",
            job_id = %params.0.job_id,
            "tool.invocation kind=act_run_shell_cancel"
        );
        if let Err(error) = self.ensure_supported_use_allows_action("act_run_shell") {
            self.audit_action_denied_for_request("act_run_shell_cancel", &error, &request_context);
            return Err(error);
        }
        let params = params.0;
        let session_id = require_shell_session_id(&request_context)?;
        let service = self.clone();
        let result = crate::m4::spawn_shell_job_blocking(move || {
            let before_status = shell_job_status(
                &ActRunShellStatusParams {
                    job_id: params.job_id.clone(),
                    tail_bytes: 1024,
                },
                Some(&session_id),
            );
            let command_payload = json!({
                "job_id": &params.job_id,
                "session_id": &session_id,
            });
            let command_before = json!({
                "source_of_truth": "durable shell registry/log files/process table",
                "status_readback": before_status.as_ref().ok(),
                "status_error": before_status.as_ref().err().map(|error| json!({
                    "code": error.data.as_ref().and_then(|data| data.get("code")).and_then(Value::as_str),
                    "message": error.message.to_string(),
                })),
            });
            service.command_audit_intent(super::command_audit::CommandAuditInput::mcp(
                "act_run_shell_cancel",
                "kill",
                Some(session_id.clone()),
                Some(session_id.clone()),
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            ))?;
            service.audit_action_started_with_details_for_session(
                "act_run_shell_cancel",
                &command_payload,
                &session_id,
            )?;
            let result = cancel_shell_job(&params, Some(&session_id));
            let after_status = shell_job_status(
                &ActRunShellStatusParams {
                    job_id: params.job_id,
                    tail_bytes: 1024,
                },
                Some(&session_id),
            );
            // #198: the command-audit final row must not skip the action-log
            // closing row; its failure surfaces after both rows are written.
            let command_final = match &result {
                Ok(response) => service.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "act_run_shell_cancel",
                        "kill",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": "durable shell registry/log files/process table",
                            "response": response,
                            "status_readback": after_status.as_ref().ok(),
                            "status_error": after_status.as_ref().err().map(|error| json!({
                                "code": error.data.as_ref().and_then(|data| data.get("code")).and_then(Value::as_str),
                                "message": error.message.to_string(),
                            })),
                        }),
                        "ok",
                    ),
                ),
                Err(error) => service.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "act_run_shell_cancel",
                        "kill",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": "durable shell registry/log files/process table",
                            "status_readback": after_status.as_ref().ok(),
                            "status_error": after_status.as_ref().err().map(|error| json!({
                                "code": error.data.as_ref().and_then(|data| data.get("code")).and_then(Value::as_str),
                                "message": error.message.to_string(),
                            })),
                        }),
                        "error",
                    )
                    .with_error(super::command_audit::command_audit_error_from_error_data(error)),
                ),
            };
            service.audit_action_result_for_session("act_run_shell_cancel", &result, &session_id)?;
            command_final?;
            result
        }).await.map_err(|error| mcp_error(error_codes::TOOL_INTERNAL_ERROR,
                format!("act_run_shell_cancel worker failed: {error}")))?;
        result.map(Json)
    }

    #[tool(
        description = "Launch an allowlisted local process, optionally on a session-owned hidden desktop, and optionally wait for a visible-desktop window when no desktop override is used"
    )]
    pub async fn act_launch(
        &self,
        params: Parameters<ActLaunchParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActLaunchResponse>, ErrorData> {
        self.act_launch_with_override(params.0, None, request_context)
            .await
    }

    #[tool(
        description = "Spawn a fully capable primary Codex, Claude, or local_model agent as a hidden background process, wire it to the configured Synapse HTTP MCP daemon, require real MCP session registration, optionally bind a per-session target, and return only after public session facade readback plus a validated task-start readiness artifact prove the spawned prompt began executing. Pass cli/kind plus a non-empty prompt for a direct spawn, or template_id (+ template_params) to render the spawn from a durable agent_template; a template-rendered spawn records the exact (template_id, version, config_hash) used and rejects passing cli/kind/model/model_ref/prompt/working_dir/target alongside the template."
    )]
    pub async fn act_spawn_agent(
        &self,
        params: Parameters<ActSpawnAgentRequest>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActSpawnAgentResponse>, ErrorData> {
        self.spawn_agent_journaled(params.0, &request_context)
            .await
            .map(Json)
    }

    #[tool(
        description = "Spawned-agent cooperative readiness: after the spawned agent has called health/session facade and verified its target, call this tool with the daemon-issued spawn_id. The daemon uses the real MCP request session id to atomically write task-started.json, avoiding permission-gated local shell readiness helpers."
    )]
    pub async fn agent_spawn_task_started(
        &self,
        params: Parameters<AgentSpawnTaskStartedParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<AgentSpawnTaskStartedResponse>, ErrorData> {
        self.agent_spawn_task_started_impl(params.0, &request_context)
            .map(Json)
    }
}

impl SynapseService {
    fn agent_spawn_task_started_impl(
        &self,
        params: AgentSpawnTaskStartedParams,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<AgentSpawnTaskStartedResponse, ErrorData> {
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::HTTP_SESSION_INVALID,
                    "agent_spawn_task_started requires an MCP session id (the daemon writes task-started.json from the real request session, not a caller-supplied value)",
                )
            })?;
        write_agent_spawn_task_started_from_session(&params.spawn_id, &session_id)
    }

    pub(crate) async fn spawn_agent_journaled(
        &self,
        request: ActSpawnAgentRequest,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<ActSpawnAgentResponse, ErrorData> {
        if let Err(error) = self.ensure_supported_use_allows_action("act_launch") {
            self.audit_action_denied_for_request(ACT_SPAWN_AGENT, &error, request_context);
            return Err(error);
        }
        // Resolve the request — direct spawn or template-rendered — into the
        // concrete spawn params before any side effect, so a bad template id or
        // param contract fails loudly with nothing launched (#909).
        let params = match self.resolve_spawn_request(request) {
            Ok(params) => params,
            Err(error) => {
                self.audit_action_denied_for_request(ACT_SPAWN_AGENT, &error, request_context);
                return Err(error);
            }
        };
        let agent_kind = params.effective_cli()?;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = ACT_SPAWN_AGENT,
            cli = agent_kind.as_str(),
            model_ref = params.local_model_ref().unwrap_or(""),
            template_id = params.template_id.as_deref().unwrap_or(""),
            template_version = params.template_version.unwrap_or(0),
            "tool.invocation kind=act_spawn_agent"
        );
        let started_by_session_id =
            super::context::mcp_session_id_from_request_context(request_context)?;
        let trail = SpawnAuditTrail::new(
            SpawnAuditSink::Request(request_context),
            &params,
            started_by_session_id,
        );
        self.spawn_agent_audited(trail, params).await
    }

    /// The one audited spawn lifecycle behind `act_spawn_agent`, task
    /// auto-dispatch and the dashboard (#198): writes the started row, then
    /// closes the command-audit and action-log rows on every outcome.
    async fn spawn_agent_audited(
        &self,
        trail: SpawnAuditTrail<'_>,
        params: ActSpawnAgentParams,
    ) -> Result<ActSpawnAgentResponse, ErrorData> {
        // D8 (#598): refused before the started row and before any launch.
        if let Err(error) = self.m4_config.agent_spawn().ensure_allowed() {
            match trail.sink {
                SpawnAuditSink::Request(request_context) => {
                    self.audit_action_denied_for_request(ACT_SPAWN_AGENT, &error, request_context);
                }
                SpawnAuditSink::Dashboard => self.audit_action_denied_with_details(
                    ACT_SPAWN_AGENT,
                    &error,
                    &json!({ "channel": "dashboard" }),
                ),
            }
            return Err(error);
        }
        match trail.sink {
            SpawnAuditSink::Request(request_context) => self
                .audit_action_started_with_details_for_request(
                    ACT_SPAWN_AGENT,
                    &trail.command_payload,
                    request_context,
                )?,
            SpawnAuditSink::Dashboard => {
                self.audit_action_started_with_details(ACT_SPAWN_AGENT, &trail.command_payload)?;
            }
        }
        let (command_before, outcome) = self.spawn_agent_after_started(&trail, params).await;
        let command_final = self.spawn_command_audit_final(&trail, command_before, &outcome);
        match trail.sink {
            SpawnAuditSink::Request(request_context) => self.audit_action_result_for_request(
                ACT_SPAWN_AGENT,
                &outcome.result,
                request_context,
            )?,
            SpawnAuditSink::Dashboard => {
                self.audit_action_result(ACT_SPAWN_AGENT, &outcome.result)?;
            }
        }
        command_final?;
        outcome.result
    }

    async fn spawn_agent_after_started(
        &self,
        trail: &SpawnAuditTrail<'_>,
        params: ActSpawnAgentParams,
    ) -> (Value, SpawnOutcome) {
        let (command_before, prepared) = self.spawn_agent_prepare(trail, &params);
        if let Err(error) = prepared {
            return (command_before, SpawnOutcome::failed(error));
        }
        let started_by = trail.started_by_session_id.clone();
        let spawned = self
            .act_spawn_agent_impl(params, started_by, trail.spawn_id.clone())
            .await;
        let outcome = match spawned {
            Ok(response) => SpawnOutcome {
                result: self
                    .journal_spawn_ready(&response)
                    .map(|()| response.clone()),
                spawned: Some(response),
            },
            Err(error) => {
                // The spawn error must win the response; the journal failure
                // (if any) is already logged as AGENT_EVENT_WRITE_FAILED.
                self.journal_spawn_failed(&trail.spawn_id, &error);
                SpawnOutcome::failed(error)
            }
        };
        (command_before, outcome)
    }

    /// Pre-launch steps after the started row: the before snapshot, the
    /// command-audit intent and the spawn-requested journal event (a spawn
    /// that cannot be journaled is refused before launching, #897).
    fn spawn_agent_prepare(
        &self,
        trail: &SpawnAuditTrail<'_>,
        params: &ActSpawnAgentParams,
    ) -> (Value, Result<(), ErrorData>) {
        let before_session_ids = self.current_session_ids();
        let command_before = trail.command_before(
            before_session_ids
                .as_ref()
                .map_or(Value::Null, |session_ids| json!(session_ids)),
        );
        let started_by = trail.started_by_session_id.as_deref();
        let prepared = before_session_ids
            .and_then(|_session_ids| {
                self.command_audit_intent(trail.command_input(
                    trail.started_by_session_id.clone(),
                    command_before.clone(),
                    Value::Null,
                    "pending",
                ))
            })
            .and_then(|_readback| {
                self.journal_spawn_requested(&trail.spawn_id, params, started_by)
            });
        (command_before, prepared)
    }

    fn spawn_command_audit_final(
        &self,
        trail: &SpawnAuditTrail<'_>,
        command_before: Value,
        outcome: &SpawnOutcome,
    ) -> Result<(), ErrorData> {
        let mut after = json!({
            "source_of_truth": SPAWN_AGENT_SOURCE_OF_TRUTH,
            "spawn_id": &trail.spawn_id,
            "after_session_ids": self.current_session_ids().unwrap_or_default(),
        });
        if let (Some(response), Some(fields)) = (&outcome.spawned, after.as_object_mut()) {
            fields.insert("response".to_owned(), json!(response));
        }
        // A spawned agent is the target even when its journal write failed.
        let target_session_id = outcome
            .spawned
            .as_ref()
            .map(|response| response.session_id.clone())
            .or_else(|| trail.started_by_session_id.clone());
        let input = match &outcome.result {
            Ok(_) => trail.command_input(target_session_id, command_before, after, "ok"),
            Err(error) => trail
                .command_input(target_session_id, command_before, after, "error")
                .with_error(super::command_audit::command_audit_error_from_error_data(
                    error,
                )),
        };
        self.command_audit_final(input).map(|_readback| ())
    }
}

impl SynapseService {
    /// `act_launch` with an optional per-call owner override (#258; the
    /// `process operation=launch` facade passes it, raw act_launch does not).
    async fn act_launch_with_override(
        &self,
        params: ActLaunchParams,
        owner_override: Option<super::owner_override::OwnerOverride>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActLaunchResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_launch",
            target = %params.target,
            "tool.invocation kind=act_launch"
        );
        if let Err(error) = self.ensure_supported_use_allows_action("act_launch") {
            self.audit_action_denied_for_request("act_launch", &error, &request_context);
            return Err(error);
        }
        let session_id = super::context::mcp_session_id_from_request_context(&request_context)?;
        if let Err(error) = self.require_launch_profile_policy(session_id.as_deref(), &params) {
            self.audit_action_denied_for_request("act_launch", &error, &request_context);
            return Err(error);
        }
        self.act_launch_for_session_id(params, session_id, owner_override.as_ref())
            .await
            .map(Json)
    }

    pub(super) async fn act_launch_for_session_id(
        &self,
        params: ActLaunchParams,
        session_id: Option<String>,
        owner_override: Option<&super::owner_override::OwnerOverride>,
    ) -> Result<ActLaunchResponse, ErrorData> {
        let mut command_payload = launch_request_details(&params);
        super::owner_override::audit_owner_override(&mut command_payload, owner_override);
        let command_before = json!({
            "source_of_truth": LAUNCH_SOURCE_OF_TRUTH,
            "session_id": &session_id,
        });
        self.command_audit_intent(launch_command_audit_input(
            session_id.as_deref(),
            command_payload.clone(),
            command_before.clone(),
            Value::Null,
            "pending",
        ))?;
        if let Some(session_id) = session_id.as_deref() {
            self.audit_action_started_with_details_for_session(
                "act_launch",
                &command_payload,
                session_id,
            )?;
        } else {
            self.audit_action_started_with_details("act_launch", &command_payload)?;
        }
        // #198: every outcome after the started row (even an unadoptable
        // spawned process) becomes `result`, so both closing rows are written.
        let result = self
            .launch_and_adopt(&params, session_id.as_deref(), owner_override)
            .await;
        let command_final = self.launch_command_audit_final(
            session_id.as_deref(),
            command_payload,
            command_before,
            &result,
        );
        if let Some(session_id) = session_id.as_deref() {
            self.audit_action_result_for_session("act_launch", &result, session_id)?;
        } else {
            self.audit_action_result("act_launch", &result)?;
        }
        command_final?;
        result
    }

    /// The user-activity gate (#258: a visible GUI launch is refused, nothing
    /// spawned, while the human is active, unless this call carries its own
    /// owner override), then the launch and its adoption.
    async fn launch_and_adopt(
        &self,
        params: &ActLaunchParams,
        session_id: Option<&str>,
        owner_override: Option<&super::owner_override::OwnerOverride>,
    ) -> Result<ActLaunchResponse, ErrorData> {
        self.ensure_user_idle_for_launch(params, session_id, owner_override)?;
        let outcome = match launch_for_session(&self.m4_config, params.clone(), session_id).await {
            Ok(outcome) => outcome,
            Err(mut error) => {
                // #2346: a launch that failed after its spawn leaves a `launch_failed` row;
                // #2401: when that row could not be written, the error says so.
                if let Some(failed) = ActLaunchResponse::from_failed_launch(&error)
                    && let Err(history_error) =
                        record_launch_process_history(self, params, &failed, "launch_failed")
                {
                    tracing::warn!(
                        code = "M4_ACT_LAUNCH_FAILED_HISTORY_NOT_RECORDED",
                        pid = failed.pid,
                        error = %history_error.message,
                        "act_launch could not record the launch_failed process history row"
                    );
                    if let Some(Value::Object(data)) = error.data.as_mut() {
                        data.insert("launch_failed_history".to_owned(), json!("not_recorded"));
                    }
                }
                return Err(error);
            }
        };
        self.adopt_launched_process(params, session_id.map(ToOwned::to_owned), outcome)
    }

    /// Records the launched process in process history and, for a session
    /// launch, registers its job as a session resource. On failure the exact
    /// spawned PID tree is terminated and the error is returned for the
    /// caller's closing audit rows.
    fn adopt_launched_process(
        &self,
        params: &ActLaunchParams,
        session_id: Option<String>,
        mut outcome: crate::m4::ActLaunchOutcome,
    ) -> Result<ActLaunchResponse, ErrorData> {
        let response = outcome.response.clone();
        // The child was confined to its session job before it ran (#40).
        #[cfg(windows)]
        if let Some(job) = outcome.process_job.as_mut() {
            let receiver = super::console_type::ConsoleType::capture_launch(&response, job);
            job.remember_console_receiver(receiver);
        }
        let process_job = outcome.process_job.take();
        if let Err(error) = record_launch_process_history(self, params, &response, "started") {
            return Err(launch_adoption_error(
                response.pid,
                "process_history_record_failed",
                "act_launch spawned the process but failed to record process history; exact spawned PID cleanup was attempted",
                &error.message,
            ));
        }
        if let (Some(session_id), Some(process_job)) = (session_id, process_job)
            && let Err(error) = self.register_session_process_resource(
                super::session_lifecycle::SessionProcessResource::new(
                    session_id,
                    "act_launch",
                    response.pid,
                    None,
                    params.target.clone(),
                    process_job,
                )
                .with_desktop_lease(outcome.desktop_lease.take()),
            )
        {
            return Err(launch_adoption_error(
                response.pid,
                "session_process_register_failed",
                "act_launch spawned the process but failed to register the session process resource; exact spawned PID cleanup was attempted",
                &error.message,
            ));
        }
        Ok(response)
    }

    fn launch_command_audit_final(
        &self,
        session_id: Option<&str>,
        command_payload: Value,
        command_before: Value,
        result: &Result<ActLaunchResponse, ErrorData>,
    ) -> Result<(), ErrorData> {
        let input = match result {
            Ok(response) => launch_command_audit_input(
                session_id,
                command_payload,
                command_before,
                json!({
                    "source_of_truth": LAUNCH_SOURCE_OF_TRUTH,
                    "response": response,
                }),
                "ok",
            ),
            Err(error) => launch_command_audit_input(
                session_id,
                command_payload,
                command_before,
                json!({ "source_of_truth": LAUNCH_SOURCE_OF_TRUTH }),
                "error",
            )
            .with_error(super::command_audit::command_audit_error_from_error_data(
                error,
            )),
        };
        self.command_audit_final(input).map(|_readback| ())
    }
}

/// Source of truth named in every `act_launch` command-audit row.
const LAUNCH_SOURCE_OF_TRUTH: &str =
    "process table plus CF_PROCESS_HISTORY/session lifecycle resources";

fn launch_command_audit_input(
    session_id: Option<&str>,
    payload: Value,
    before: Value,
    after: Value,
    outcome: &'static str,
) -> super::command_audit::CommandAuditInput {
    super::command_audit::CommandAuditInput::mcp(
        "act_launch",
        "spawn",
        session_id.map(str::to_owned),
        session_id.map(str::to_owned),
        payload,
        before,
        after,
        outcome,
    )
}

/// Terminates the exact spawned PID tree of a launch that could not be
/// adopted and describes the failure (with the cleanup readback).
fn launch_adoption_error(
    pid: u32,
    reason: &'static str,
    message: &'static str,
    source_error: &str,
) -> ErrorData {
    let cleanup = crate::m4::terminate_owned_process_tree(pid);
    launch_lifecycle_tool_error(
        message,
        json!({
            "code": error_codes::TOOL_INTERNAL_ERROR,
            "reason": reason,
            "pid": pid,
            "source_error": source_error,
            "cleanup": cleanup,
        }),
    )
}

/// Source of truth named in every `act_spawn_agent` command-audit row.
const SPAWN_AGENT_SOURCE_OF_TRUTH: &str =
    "CF_AGENT_EVENTS, CF_PROCESS_HISTORY, session registry, agent spawn artifacts";

/// Where one spawn's `act_spawn_agent` action-log rows are written.
#[derive(Clone, Copy)]
enum SpawnAuditSink<'a> {
    /// An MCP request (tool call or task auto-dispatch): rows carry its session.
    Request(&'a RequestContext<RoleServer>),
    /// The dashboard channel: daemon rows; command-audit rows tagged `dashboard`.
    Dashboard,
}

/// The audit identity of one spawn lifecycle. The spawn id is allocated
/// before any side effect so every journal event of the lifecycle (#897)
/// shares one attribution anchor.
struct SpawnAuditTrail<'a> {
    sink: SpawnAuditSink<'a>,
    started_by_session_id: Option<String>,
    spawn_id: String,
    command_payload: Value,
}

impl<'a> SpawnAuditTrail<'a> {
    fn new(
        sink: SpawnAuditSink<'a>,
        params: &ActSpawnAgentParams,
        started_by_session_id: Option<String>,
    ) -> Self {
        Self {
            sink,
            command_payload: agent_spawn_request_details(params, started_by_session_id.as_deref()),
            started_by_session_id,
            spawn_id: format!("agent-spawn-{}", new_reflex_id()),
        }
    }

    fn command_before(&self, before_session_ids: Value) -> Value {
        let mut before = json!({
            "source_of_truth": SPAWN_AGENT_SOURCE_OF_TRUTH,
            "spawn_id": &self.spawn_id,
            "before_session_ids": before_session_ids,
        });
        if let (SpawnAuditSink::Dashboard, Some(fields)) = (self.sink, before.as_object_mut()) {
            fields.insert("dashboard_channel".to_owned(), Value::Bool(true));
        }
        before
    }

    fn command_input(
        &self,
        target_session_id: Option<String>,
        before: Value,
        after: Value,
        outcome: &'static str,
    ) -> super::command_audit::CommandAuditInput {
        let input = super::command_audit::CommandAuditInput::mcp(
            ACT_SPAWN_AGENT,
            "spawn",
            self.started_by_session_id.clone(),
            target_session_id,
            self.command_payload.clone(),
            before,
            after,
            outcome,
        );
        match self.sink {
            SpawnAuditSink::Request(_) => input,
            SpawnAuditSink::Dashboard => input.with_channel("dashboard"),
        }
    }
}

/// What a spawn produced: its result, plus the response whenever an agent was
/// really spawned (kept even when the post-spawn journal write failed).
struct SpawnOutcome {
    result: Result<ActSpawnAgentResponse, ErrorData>,
    spawned: Option<ActSpawnAgentResponse>,
}

impl SpawnOutcome {
    const fn failed(error: ErrorData) -> Self {
        Self {
            result: Err(error),
            spawned: None,
        }
    }
}

fn record_launch_process_history(
    service: &SynapseService,
    params: &ActLaunchParams,
    response: &ActLaunchResponse,
    status: &str,
) -> Result<(), ErrorData> {
    let row = launch_process_history_row(params, response, status)?;
    let row_key = launch_process_history_row_key(response);
    let runtime = service.reflex_runtime()?;
    let runtime = runtime.lock().map_err(|_error| {
        mcp_error(
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            "reflex runtime lock poisoned while recording act_launch process history",
        )
    })?;
    runtime
        .storage_put_process_history_rows(vec![(row_key, row)])
        .map_err(|error| mcp_error(error.code(), error.to_string()))
}

fn launch_agent_spawn_with_terminal_capture(
    config: &crate::m4::M4ServiceConfig,
    params: &ActLaunchParams,
    files: &AgentSpawnFiles,
    spawn_id: &str,
    launched_at_unix_ms: u64,
) -> Result<(ActLaunchResponse, crate::m4::OwnedProcessJob), ErrorData> {
    crate::m4::validate_launch_authorized(config, params)?;
    let env = crate::m4::child_environment::spawned_agent_environment(params)?;
    let artifacts = agent_spawn_terminal_capture_artifacts(&files.log_dir);
    let spec = CaptureSpec {
        live_key: Some(spawn_id.to_owned()),
        program: params.target.clone(),
        args: params.args.clone(),
        cwd: params.working_dir.as_ref().map(PathBuf::from),
        env,
        cols: 120,
        rows: 40,
        started_unix_secs: launched_at_unix_ms / 1000,
        title: Some(format!("Synapse {spawn_id}")),
    };
    let (spawned, process_job) = spawn_capture_to_asciicast_with_job(
        spec,
        artifacts.clone(),
        spawn_id,
        assign_owned_process_job,
    )
    .map_err(|error| {
        agent_spawn_tool_error(
            error_codes::ACTION_AGENT_SPAWN_FAILED,
            "act_spawn_agent failed to start the spawned agent in an owned PTY terminal",
            json!({
                "code": error_codes::ACTION_AGENT_SPAWN_FAILED,
                "reason": "owned_pty_spawn_failed",
                "spawn_id": spawn_id,
                "launch_target": params.target,
                "log_dir": files.log_dir.display().to_string(),
                "terminal_asciicast_path": artifacts.asciicast_path.display().to_string(),
                "terminal_capture_status_path": artifacts.status_path.display().to_string(),
                "terminal_final_screen_path": artifacts.final_screen_path.display().to_string(),
                "terminal_input_audit_path": artifacts.input_audit_path.display().to_string(),
                "source_error": error.to_string(),
            }),
        )
    })?;
    tracing::info!(
        code = "AGENT_SPAWN_OWNED_PTY_STARTED",
        spawn_id,
        pid = spawned.process_id,
        asciicast_path = %spawned.artifacts.asciicast_path.display(),
        status_path = %spawned.artifacts.status_path.display(),
        "act_spawn_agent launched wrapper in an owned PTY"
    );
    Ok((
        ActLaunchResponse {
            pid: spawned.process_id,
            hwnd: None,
            window_owner_pid: None,
            reused_existing_window: false,
            window_owner_rule: None,
            matched_title: None,
            launched_at: chrono::Utc::now().to_rfc3339(),
            reason: Some("owned_conpty_terminal_capture".to_owned()),
            cdp_debug_port: None,
            cdp_endpoint: None,
            cdp_user_data_dir: None,
            cdp_verified_url: None,
            cdp_verified_title: None,
            desktop: None,
        },
        process_job,
    ))
}

fn require_shell_session_id(
    request_context: &RequestContext<RoleServer>,
) -> Result<String, ErrorData> {
    super::context::mcp_session_id_from_request_context(request_context)?.ok_or_else(|| {
        mcp_error(
            error_codes::HTTP_SESSION_INVALID,
            "act_run_shell tools require an MCP session id (run the daemon in HTTP mode so each agent has its own Mcp-Session-Id)",
        )
    })
}

impl SynapseService {
    /// #21: launching a shell interpreter is shell execution, so it follows
    /// the same profile policy as the `shell` tool. Other launches pass.
    fn require_launch_profile_policy(
        &self,
        session_id: Option<&str>,
        params: &ActLaunchParams,
    ) -> Result<(), ErrorData> {
        let profile = self.tool_profile_snapshot(session_id)?.profile;
        if self.m4_config.read_only_commands().allows_process_launch(
            profile.as_str(),
            &params.target,
            &params.args,
            !params.env.is_empty(),
            params.working_dir.is_some(),
        ) {
            return Ok(());
        }
        Err(mcp_error(
            error_codes::TOOL_PROFILE_POLICY_DENIED,
            format!(
                "launching shell interpreter {} is not permitted for profile {}; it follows the shell command policy. Use an allowlisted command or obtain break_glass/full_capability through the profile policy",
                params.target,
                profile.as_str()
            ),
        ))
    }

    fn require_shell_command_profile_policy(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        has_env_overrides: bool,
        has_working_dir_override: bool,
    ) -> Result<(), ErrorData> {
        let profile = self.tool_profile_snapshot(Some(session_id))?.profile;
        if self.m4_config.read_only_commands().allows_shell_command(
            profile.as_str(),
            command,
            args,
            has_env_overrides,
            has_working_dir_override,
        ) {
            return Ok(());
        }
        Err(mcp_error(
            error_codes::TOOL_PROFILE_POLICY_DENIED,
            format!(
                "shell command is not permitted for profile {}; use an allowlisted command or obtain break_glass/full_capability through the profile policy",
                profile.as_str()
            ),
        ))
    }

    pub(crate) async fn dashboard_spawn_local_model_agent(
        &self,
        params: ActSpawnAgentParams,
    ) -> Result<ActSpawnAgentResponse, ErrorData> {
        tracing::info!(
            code = "DASHBOARD_LOCAL_MODEL_SPAWN_REQUESTED",
            kind = ACT_SPAWN_AGENT,
            "dashboard.invocation kind=act_spawn_agent"
        );
        if let Err(error) = self.ensure_supported_use_allows_action("act_launch") {
            self.audit_action_denied_with_details(
                ACT_SPAWN_AGENT,
                &error,
                &json!({
                    "channel": "dashboard",
                    "source": "dashboard_local_model_spawn",
                }),
            );
            return Err(error);
        }
        let agent_kind = params.effective_cli()?;
        if !agent_kind.is_local_model() {
            let error = mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "dashboard local-model spawn requires kind=local_model",
            );
            self.audit_action_denied_with_details(
                ACT_SPAWN_AGENT,
                &error,
                &json!({
                    "channel": "dashboard",
                    "source": "dashboard_local_model_spawn",
                    "requested_kind": agent_kind.as_str(),
                }),
            );
            return Err(error);
        }
        let trail = SpawnAuditTrail::new(SpawnAuditSink::Dashboard, &params, None);
        self.spawn_agent_audited(trail, params).await
    }

    pub(crate) async fn dashboard_spawn_agent_request(
        &self,
        request: ActSpawnAgentRequest,
    ) -> Result<ActSpawnAgentResponse, ErrorData> {
        tracing::info!(
            code = "DASHBOARD_AGENT_SPAWN_REQUESTED",
            kind = ACT_SPAWN_AGENT,
            template_id = request.template_id.as_deref().unwrap_or(""),
            "dashboard.invocation kind=act_spawn_agent"
        );
        if let Err(error) = self.ensure_supported_use_allows_action("act_launch") {
            self.audit_action_denied_with_details(
                ACT_SPAWN_AGENT,
                &error,
                &json!({
                    "channel": "dashboard",
                    "source": "dashboard_spawn_agent",
                }),
            );
            return Err(error);
        }
        let params = match self.resolve_spawn_request(request) {
            Ok(params) => params,
            Err(error) => {
                self.audit_action_denied_with_details(
                    ACT_SPAWN_AGENT,
                    &error,
                    &json!({
                        "channel": "dashboard",
                        "source": "dashboard_spawn_agent",
                    }),
                );
                return Err(error);
            }
        };
        let agent_kind = params.effective_cli()?;
        tracing::info!(
            code = "DASHBOARD_AGENT_SPAWN_RESOLVED",
            cli = agent_kind.as_str(),
            model_ref = params.local_model_ref().unwrap_or(""),
            template_id = params.template_id.as_deref().unwrap_or(""),
            template_version = params.template_version.unwrap_or(0),
            "dashboard.invocation kind=act_spawn_agent resolved"
        );
        let trail = SpawnAuditTrail::new(SpawnAuditSink::Dashboard, &params, None);
        self.spawn_agent_audited(trail, params).await
    }

    /// Registers an OpenAI-compatible API model (e.g. DeepSeek) from the
    /// dashboard channel. This is the same registry the MCP `local_model_register`
    /// tool writes to (CF_KV `local_model_registry/v1/...`); registration runs
    /// the forced structured tool-call probe against the live endpoint before
    /// persisting, so a model that cannot tool-call (or whose key is missing) is
    /// rejected loudly here rather than failing on first spawn. The credential
    /// itself is never stored — only the `api_key_env_var` name is — and the
    /// daemon must already have that env var set (the spawn path forwards it into
    /// the agent child via [`resolve_spawn_local_model_api_key`]).
    pub(crate) async fn dashboard_register_api_model(
        &self,
        params: crate::m3::local_models::LocalModelRegisterParams,
    ) -> Result<crate::m3::local_models::LocalModelRegisterResponse, ErrorData> {
        tracing::info!(
            code = "DASHBOARD_API_MODEL_REGISTER_REQUESTED",
            name = %params.name,
            model_id = %params.model_id,
            allow_non_loopback = params.allow_non_loopback,
            has_api_key_env_var = params.api_key_env_var.is_some(),
            "dashboard.invocation kind=local_model_register"
        );
        let db = self.m3_storage()?;
        crate::m3::local_models::register_local_model(&db, params, "dashboard").await
    }

    /// Lists the local/API model registry rows for the dashboard spawn-console
    /// model picker. Reads the same CF_KV source of truth as `local_model_list`.
    pub(crate) fn dashboard_list_local_models(
        &self,
    ) -> Result<crate::m3::local_models::LocalModelListResponse, ErrorData> {
        let db = self.m3_storage()?;
        crate::m3::local_models::list_local_models(
            &db,
            &crate::m3::local_models::LocalModelListParams {
                name: None,
                include_disabled: true,
                limit: 1000,
            },
        )
    }

    /// Re-probes a single model-registry row from the dashboard. This persists
    /// the same probe evidence as the MCP `local_model_probe` tool, keeping the
    /// model table's manual action tied to the CF_KV registry source of truth.
    pub(crate) async fn dashboard_probe_local_model(
        &self,
        params: crate::m3::local_models::LocalModelProbeParams,
    ) -> Result<crate::m3::local_models::LocalModelProbeResponse, ErrorData> {
        tracing::info!(
            code = "DASHBOARD_MODEL_PROBE_REQUESTED",
            name = %params.name,
            timeout_ms = ?params.timeout_ms,
            "dashboard.invocation kind=local_model_probe"
        );
        let db = self.m3_storage()?;
        crate::m3::local_models::probe_local_model(&db, &params, "dashboard").await
    }

    /// Removes a model-registry row (and any stored encrypted key) for the
    /// dashboard model manager. Same CF_KV source of truth as `local_model_remove`.
    pub(crate) fn dashboard_remove_local_model(
        &self,
        params: crate::m3::local_models::LocalModelRemoveParams,
    ) -> Result<crate::m3::local_models::LocalModelRemoveResponse, ErrorData> {
        tracing::info!(
            code = "DASHBOARD_MODEL_REMOVE_REQUESTED",
            name = %params.name,
            "dashboard.invocation kind=local_model_remove"
        );
        let db = self.m3_storage()?;
        crate::m3::local_models::remove_local_model(&db, &params)
    }

    /// Updates a model-registry row (rename, edit fields, (re)store/clear the
    /// API key, enable/disable) for the dashboard model manager, re-probing as
    /// `update_local_model` requires. Same CF_KV source of truth.
    pub(crate) async fn dashboard_update_local_model(
        &self,
        params: crate::m3::local_models::LocalModelUpdateParams,
    ) -> Result<crate::m3::local_models::LocalModelUpdateResponse, ErrorData> {
        tracing::info!(
            code = "DASHBOARD_MODEL_UPDATE_REQUESTED",
            name = %params.name,
            new_name = params.new_name.as_deref().unwrap_or(""),
            has_api_key = params.api_key.is_some(),
            clear_api_key = params.clear_api_key,
            enabled = ?params.enabled,
            "dashboard.invocation kind=local_model_update"
        );
        let db = self.m3_storage()?;
        crate::m3::local_models::update_local_model(&db, params, "dashboard").await
    }

    /// Resolves a caller's spawn request into concrete spawn params. A direct
    /// spawn (no `template_id`) passes its fields through; a template spawn
    /// renders the params atomically from the durable template and stamps the
    /// `(id, version, config_hash)` provenance. The two modes are mutually
    /// exclusive and conflicts are rejected loudly — never silently merged.
    pub(crate) fn resolve_spawn_request(
        &self,
        request: ActSpawnAgentRequest,
    ) -> Result<ActSpawnAgentParams, ErrorData> {
        let daemon_bind = self.m3_bind_addr()?;
        match request.template_id {
            Some(template_id) => {
                let mut conflicts = Vec::new();
                if request.cli.is_some() {
                    conflicts.push("cli");
                }
                if request.kind.is_some() {
                    conflicts.push("kind");
                }
                if request.model.is_some() {
                    conflicts.push("model");
                }
                if request.model_ref.is_some() {
                    conflicts.push("model_ref");
                }
                if request.prompt.is_some() {
                    conflicts.push("prompt");
                }
                if request.working_dir.is_some() {
                    conflicts.push("working_dir");
                }
                if request.target.is_some() {
                    conflicts.push("target");
                }
                if !conflicts.is_empty() {
                    return Err(mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        format!(
                            "act_spawn_agent renders {conflicts:?} from template {template_id:?}; edit the template instead of passing these fields alongside template_id"
                        ),
                    ));
                }
                let rendered = self.resolve_spawn_template(
                    &template_id,
                    request.template_version,
                    &request.template_params,
                )?;
                Ok(ActSpawnAgentParams {
                    cli: Some(rendered.cli),
                    kind: Some(rendered.cli),
                    model: rendered.model,
                    model_ref: rendered.model_ref,
                    prompt: rendered.prompt,
                    target: rendered.target,
                    working_dir: rendered.working_dir,
                    mcp_url: crate::m4::agent_mcp_url(request.mcp_url.as_deref(), &daemon_bind),
                    wait_timeout_ms: request.wait_timeout_ms,
                    hold_open_ms: request.hold_open_ms,
                    require_approval_gate: request.require_approval_gate,
                    template_id: Some(rendered.provenance.template_id),
                    template_version: Some(rendered.provenance.version),
                    template_config_hash: Some(rendered.provenance.config_hash),
                })
            }
            None => request.into_direct_spawn_params(&daemon_bind),
        }
    }

    async fn act_spawn_agent_impl(
        &self,
        params: ActSpawnAgentParams,
        started_by_session_id: Option<String>,
        spawn_id: String,
    ) -> Result<ActSpawnAgentResponse, ErrorData> {
        validate_agent_spawn_params(&params)?;
        validate_spawn_target(&params.target)?;
        let agent_kind = params.effective_cli()?;
        validate_cli_model_before_spawn(&params, agent_kind)?;
        let local_model_row = if agent_kind.is_local_model() {
            Some(
                self.require_spawn_local_model_row(
                    &params,
                    started_by_session_id
                        .as_deref()
                        .unwrap_or("dashboard_spawn_agent"),
                )
                .await?,
            )
        } else {
            None
        };
        let mut timing = AgentSpawnTiming::new();
        let in_flight = AgentSpawnInFlightGuard::enter(&spawn_id, agent_kind);

        let orphan_recovery = recover_orphaned_agent_spawn_terminal_artifacts()?;
        if orphan_recovery.recovered_count > 0 {
            tracing::warn!(
                code = "AGENT_SPAWN_ORPHAN_RECOVERY",
                ?orphan_recovery,
                "act_spawn_agent recovered stale non-terminal agent spawn artifacts before launching a new agent"
            );
        }

        let working_dir = resolve_agent_working_dir(params.working_dir.as_deref())?;
        let before_session_ids = self.current_session_ids()?;
        let launched_at_unix_ms = unix_time_ms_now();
        let files = prepare_agent_spawn_files(&spawn_id, &params, &working_dir)?;
        let script = agent_spawn_powershell_script(&params, &files, &working_dir)?;
        let launch_host = match resolve_agent_spawn_powershell_host() {
            Ok(launch_host) => launch_host,
            Err(error) => {
                let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                    &files,
                    &params,
                    &spawn_id,
                    "failed",
                    "agent spawn PowerShell host preflight failed before launching a child process",
                    json!({
                        "reason": "agent_spawn_shell_preflight_failed",
                        "source_error_message": error.message.clone(),
                        "source_error_data": error.data,
                    }),
                );
                return Err(augment_agent_spawn_error_with_artifacts(
                    error,
                    &files,
                    &params,
                    &spawn_id,
                    "agent_spawn_shell_preflight_failed",
                    None,
                    completion_artifacts,
                ));
            }
        };

        let mut env = BTreeMap::new();
        // #22: never give a spawned agent the daemon master credential. It gets a
        // per-spawn scoped credential under the name its MCP config, hooks, and
        // runners read; dropping the lease on any failure path revokes it.
        let spawn_credential = crate::spawn_credentials::SpawnCredentialLease::issue(&spawn_id)
            .map_err(|error| {
                mcp_error(
                    error_codes::STORAGE_READ_FAILED,
                    format!("act_spawn_agent could not issue a scoped credential: {error}"),
                )
            })?;
        env.insert(
            crate::spawn_credentials::SPAWN_CREDENTIAL_ENV.to_owned(),
            spawn_credential.credential().to_owned(),
        );
        if agent_kind.is_local_model() {
            env.insert(
                crate::local_agent_capability::ENV.to_owned(),
                crate::local_agent_capability::token().to_owned(),
            );
        }
        env.insert("SYNAPSE_AGENT_SPAWN_ID".to_owned(), spawn_id.clone());
        env.insert(
            "SYNAPSE_AGENT_KIND".to_owned(),
            agent_kind.as_str().to_owned(),
        );
        if let Some(model_ref) = params.local_model_ref() {
            env.insert("SYNAPSE_AGENT_MODEL_REF".to_owned(), model_ref.to_owned());
        }
        env.insert(
            "SYNAPSE_AGENT_SPAWN_LAUNCH_TARGET".to_owned(),
            launch_host.target.clone(),
        );
        env.insert(
            "SYNAPSE_AGENT_SPAWN_LAUNCH_SOURCE".to_owned(),
            launch_host.source.clone(),
        );
        env.insert("SYNAPSE_MCP_URL".to_owned(), params.mcp_url.clone());
        env.insert("PYTHONUTF8".to_owned(), "1".to_owned());
        env.insert("PYTHONIOENCODING".to_owned(), "utf-8".to_owned());
        env.insert("PYTHONUNBUFFERED".to_owned(), "1".to_owned());

        // Forward the resolved local-model API key into the spawned agent's
        // environment. act_launch clears the child environment (env_clear) and
        // only re-applies a curated allow-list plus this map, so a remote
        // API-backed model (e.g. DeepSeek over https) would otherwise reach its
        // endpoint with no credentials and fail with HTTP 401 mid-run. Resolve
        // it from the daemon environment now and refuse the spawn loudly if the
        // row declares a key the daemon does not have, so the failure surfaces
        // at the spawn boundary with a remediation hint rather than after a
        // launched agent has already started burning a turn. Only the value is
        // forwarded into the child process env (never persisted): the process
        // history row records env keys only (m4::launch_process_history_row).
        if let Some((env_var, value)) =
            resolve_spawn_local_model_api_key(&self.m3_storage()?, local_model_row.as_ref())?
        {
            env.insert(env_var, value);
        }

        let launch_params = ActLaunchParams {
            target: launch_host.target.clone(),
            args: vec![
                "-NoLogo".to_owned(),
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-Command".to_owned(),
                script,
            ],
            working_dir: Some(working_dir.display().to_string()),
            env,
            wait_for_window_title_regex: None,
            timeout_ms: 10_000,
            idempotency_key: None,
            cdp_debug: Some(false),
            force_renderer_accessibility: None,
            windows_console_window_state: Some(LaunchWindowState::Hidden),
            desktop: None,
        };

        timing.mark_prelaunch_done();
        timing.mark_launch_started();
        let (launch_response, process_job) = match launch_agent_spawn_with_terminal_capture(
            &self.m4_config,
            &launch_params,
            &files,
            &spawn_id,
            launched_at_unix_ms,
        ) {
            Ok(response) => response,
            Err(error) => {
                let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                    &files,
                    &params,
                    &spawn_id,
                    "failed",
                    "agent spawn PowerShell host launch failed before MCP session registration",
                    json!({
                        "reason": "agent_spawn_shell_launch_failed",
                        "launch_host": launch_host.to_json(),
                        "source_error_message": error.message.clone(),
                        "source_error_data": error.data,
                        "spawn_timing": timing.readback(&in_flight, params.wait_timeout_ms),
                    }),
                );
                return Err(augment_agent_spawn_error_with_artifacts(
                    error,
                    &files,
                    &params,
                    &spawn_id,
                    "agent_spawn_shell_launch_failed",
                    Some(&launch_host.target),
                    completion_artifacts,
                ));
            }
        };
        timing.mark_launch_completed();
        if let Err(error) =
            record_launch_process_history(self, &launch_params, &launch_response, "started")
        {
            let cleanup = crate::m4::terminate_owned_process_tree(launch_response.pid);
            let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                &files,
                &params,
                &spawn_id,
                "failed",
                "process history recording failed before spawned agent completion",
                json!({
                    "reason": "process_history_record_failed",
                    "source_error": error.message,
                    "cleanup": cleanup,
                }),
            );
            return Err(agent_spawn_tool_error(
                error_codes::ACTION_AGENT_SPAWN_FAILED,
                "act_spawn_agent spawned the wrapper but failed to record process history; exact spawned PID cleanup was attempted",
                json!({
                    "code": error_codes::ACTION_AGENT_SPAWN_FAILED,
                    "reason": "process_history_record_failed",
                    "spawn_id": spawn_id,
                    "cli": agent_kind.as_str(),
                    "launcher_process_id": launch_response.pid,
                    "log_dir": files.log_dir.display().to_string(),
                    "source_error": error.message,
                    "cleanup": cleanup,
                    "completion_artifacts": completion_artifacts,
                }),
            ));
        }

        timing.mark_session_wait_started();
        let session_wait_deadline =
            agent_spawn_wait_deadline_from(Instant::now(), params.wait_timeout_ms)?;
        let mut matched = match self
            .wait_for_spawned_agent_session(
                &params,
                agent_kind,
                &spawn_id,
                &before_session_ids,
                launched_at_unix_ms,
                launch_response.pid,
                &files,
                session_wait_deadline,
            )
            .await
        {
            Ok(matched) => matched,
            Err(error) => {
                let terminal_failure =
                    error.get("reason").and_then(Value::as_str) == Some("spawn_terminal_failure");
                let (code, status, reason, message) = if terminal_failure {
                    (
                        error_codes::ACTION_AGENT_SPAWN_FAILED,
                        "failed",
                        "spawn_terminal_failure",
                        "spawned agent reported terminal failure before session readiness; exact spawned PID cleanup was attempted",
                    )
                } else {
                    (
                        error_codes::ACTION_AGENT_SPAWN_SESSION_TIMEOUT,
                        "session_timeout",
                        "session_registry_readback_timeout",
                        "act_spawn_agent did not observe a fully provisioned MCP session before timeout; exact spawned PID cleanup was attempted",
                    )
                };
                let cleanup = crate::m4::terminate_owned_process_tree(launch_response.pid);
                let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                    &files,
                    &params,
                    &spawn_id,
                    status,
                    message,
                    json!({
                        "reason": reason,
                        "wait_timeout_ms": params.wait_timeout_ms,
                        "wait_error": error,
                        "spawn_timing": timing.readback(&in_flight, params.wait_timeout_ms),
                        "cleanup": cleanup,
                    }),
                );
                return Err(agent_spawn_tool_error(
                    code,
                    message,
                    json!({
                        "code": code,
                        "reason": reason,
                        "spawn_id": spawn_id,
                        "cli": agent_kind.as_str(),
                        "launcher_process_id": launch_response.pid,
                        "mcp_url": params.mcp_url,
                        "wait_timeout_ms": params.wait_timeout_ms,
                        "target": params.target,
                        "log_dir": files.log_dir.display().to_string(),
                        "readiness_files": agent_spawn_readiness_file_readback(&files),
                        "stdout_tail": public_agent_log_tail(&files.stdout_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                        "stderr_tail": public_agent_log_tail(&files.stderr_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                        "final_message_tail": public_agent_log_tail(&files.final_message_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                        "spawn_timing": timing.readback(&in_flight, params.wait_timeout_ms),
                        "wait_error": error,
                        "cleanup": cleanup,
                        "completion_artifacts": completion_artifacts,
                    }),
                ));
            }
        };
        timing.mark_session_matched();

        timing.mark_task_wait_started();
        let task_wait_deadline =
            agent_spawn_wait_deadline_from(Instant::now(), params.wait_timeout_ms)?;
        let task_started = match self
            .wait_for_spawned_agent_task_started(
                &params,
                agent_kind,
                &spawn_id,
                &mut matched,
                &before_session_ids,
                launched_at_unix_ms,
                launch_response.pid,
                &files,
                task_wait_deadline,
            )
            .await
        {
            Ok(task_started) => task_started,
            Err(error) => {
                let terminal_failure =
                    error.get("reason").and_then(Value::as_str) == Some("spawn_terminal_failure");
                let (code, status, reason, message) = if terminal_failure {
                    (
                        error_codes::ACTION_AGENT_SPAWN_FAILED,
                        "failed",
                        "spawn_terminal_failure",
                        "spawned agent reported terminal failure before task readiness; exact spawned PID cleanup was attempted",
                    )
                } else {
                    (
                        error_codes::ACTION_AGENT_SPAWN_TASK_NOT_STARTED,
                        "task_not_started",
                        "task_start_readiness_readback_failed",
                        "act_spawn_agent observed the spawned session but did not observe task-start readiness before timeout; exact spawned PID cleanup was attempted",
                    )
                };
                let cleanup = crate::m4::terminate_owned_process_tree(launch_response.pid);
                let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                    &files,
                    &params,
                    &spawn_id,
                    status,
                    message,
                    json!({
                        "reason": reason,
                        "wait_timeout_ms": params.wait_timeout_ms,
                        "task_start_error": error,
                        "session_id": matched.session_id,
                        "spawn_timing": timing.readback(&in_flight, params.wait_timeout_ms),
                        "cleanup": cleanup,
                    }),
                );
                return Err(agent_spawn_tool_error(
                    code,
                    message,
                    json!({
                        "code": code,
                        "reason": reason,
                        "spawn_id": spawn_id,
                        "cli": agent_kind.as_str(),
                        "launcher_process_id": launch_response.pid,
                        "agent_process_id": matched.agent_process_id,
                        "session_id": matched.session_id,
                        "mcp_url": params.mcp_url,
                        "wait_timeout_ms": params.wait_timeout_ms,
                        "target": params.target,
                        "log_dir": files.log_dir.display().to_string(),
                        "task_started_path": files.task_started_path.display().to_string(),
                        "readiness_files": agent_spawn_readiness_file_readback(&files),
                        "stdout_tail": public_agent_log_tail(&files.stdout_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                        "stderr_tail": public_agent_log_tail(&files.stderr_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                        "final_message_tail": public_agent_log_tail(&files.final_message_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                        "spawn_timing": timing.readback(&in_flight, params.wait_timeout_ms),
                        "task_start_error": error,
                        "cleanup": cleanup,
                        "completion_artifacts": completion_artifacts,
                    }),
                ));
            }
        };
        timing.mark_task_started();
        if let Err(error) =
            self.require_spawned_agent_session_live(&matched.session_id, &files, agent_kind)
        {
            let cleanup = crate::m4::terminate_owned_process_tree(launch_response.pid);
            let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                &files,
                &params,
                &spawn_id,
                "task_not_started",
                "spawned agent session ended before durable spawn readiness could be returned",
                json!({
                    "reason": "spawned_session_ended_before_ready",
                    "wait_timeout_ms": params.wait_timeout_ms,
                    "session_id": matched.session_id,
                    "session_liveness_error": error,
                    "cleanup": cleanup,
                }),
            );
            return Err(agent_spawn_tool_error(
                error_codes::ACTION_AGENT_SPAWN_TASK_NOT_STARTED,
                "act_spawn_agent observed task-start readiness but the spawned MCP session was no longer live before ready could be returned; exact spawned PID cleanup was attempted",
                json!({
                    "code": error_codes::ACTION_AGENT_SPAWN_TASK_NOT_STARTED,
                    "reason": "spawned_session_ended_before_ready",
                    "spawn_id": spawn_id,
                    "cli": agent_kind.as_str(),
                    "launcher_process_id": launch_response.pid,
                    "agent_process_id": matched.agent_process_id,
                    "session_id": matched.session_id,
                    "mcp_url": params.mcp_url,
                    "wait_timeout_ms": params.wait_timeout_ms,
                    "target": params.target,
                    "log_dir": files.log_dir.display().to_string(),
                    "task_started_path": files.task_started_path.display().to_string(),
                    "readiness_files": agent_spawn_readiness_file_readback(&files),
                    "session_liveness_error": error,
                    "cleanup": cleanup,
                    "completion_artifacts": completion_artifacts,
                }),
            ));
        }
        let control = match read_spawned_agent_control_artifact(&files, agent_kind) {
            Ok(control) => control,
            Err(error) => {
                let cleanup = crate::m4::terminate_owned_process_tree(launch_response.pid);
                let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                    &files,
                    &params,
                    &spawn_id,
                    "failed",
                    "spawned agent wrote task-start readiness but the interrupt-control artifact was missing or invalid",
                    json!({
                        "reason": "spawned_agent_control_readback_failed",
                        "control_error": error,
                        "session_id": matched.session_id,
                        "cleanup": cleanup,
                    }),
                );
                return Err(agent_spawn_tool_error(
                    error_codes::ACTION_AGENT_SPAWN_FAILED,
                    "act_spawn_agent observed task-start readiness but failed to read Codex interrupt-control metadata; exact spawned PID cleanup was attempted",
                    json!({
                        "code": error_codes::ACTION_AGENT_SPAWN_FAILED,
                        "reason": "spawned_agent_control_readback_failed",
                        "spawn_id": spawn_id,
                        "cli": agent_kind.as_str(),
                        "launcher_process_id": launch_response.pid,
                        "agent_process_id": matched.agent_process_id,
                        "session_id": matched.session_id,
                        "log_dir": files.log_dir.display().to_string(),
                        "control_error": error,
                        "cleanup": cleanup,
                        "completion_artifacts": completion_artifacts,
                    }),
                ));
            }
        };

        let metadata = SpawnedAgentRead {
            spawn_id: spawn_id.clone(),
            cli: agent_kind.as_str().to_owned(),
            launcher_process_id: launch_response.pid,
            agent_process_id: matched.agent_process_id,
            started_by_session_id,
            launched_at_unix_ms,
            launch_target: launch_params.target.clone(),
            log_dir: files.log_dir.display().to_string(),
            template_id: params.template_id.clone(),
            template_version: params.template_version,
            control,
        };
        if let Err(error) = self.record_spawned_agent_metadata(&matched.session_id, metadata) {
            let cleanup = crate::m4::terminate_owned_process_tree(launch_response.pid);
            let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                &files,
                &params,
                &spawn_id,
                "failed",
                "spawned agent metadata recording failed before spawned agent completion",
                json!({
                    "reason": "spawned_agent_metadata_record_failed",
                    "source_error": error.message,
                    "session_id": matched.session_id,
                    "cleanup": cleanup,
                }),
            );
            return Err(agent_spawn_tool_error(
                error_codes::ACTION_AGENT_SPAWN_FAILED,
                "act_spawn_agent observed the spawned session but failed to record session metadata; exact spawned PID cleanup was attempted",
                json!({
                    "code": error_codes::ACTION_AGENT_SPAWN_FAILED,
                    "reason": "spawned_agent_metadata_record_failed",
                    "spawn_id": spawn_id,
                    "cli": agent_kind.as_str(),
                    "launcher_process_id": launch_response.pid,
                    "agent_process_id": matched.agent_process_id,
                    "session_id": matched.session_id,
                    "log_dir": files.log_dir.display().to_string(),
                    "source_error": error.message,
                    "cleanup": cleanup,
                    "completion_artifacts": completion_artifacts,
                }),
            ));
        }
        if let Err(error) = self.register_session_process_resource(
            super::session_lifecycle::SessionProcessResource::new(
                matched.session_id.clone(),
                ACT_SPAWN_AGENT,
                launch_response.pid,
                Some(spawn_id.clone()),
                launch_params.target.clone(),
                process_job,
            )
            .with_agent_cli(agent_kind.as_str()),
        ) {
            let cleanup = crate::m4::terminate_owned_process_tree(launch_response.pid);
            let completion_artifacts = write_agent_spawn_daemon_terminal_artifacts(
                &files,
                &params,
                &spawn_id,
                "failed",
                "session process resource registration failed before spawned agent completion",
                json!({
                    "reason": "session_process_register_failed",
                    "source_error": error.message,
                    "session_id": matched.session_id,
                    "cleanup": cleanup,
                }),
            );
            return Err(agent_spawn_tool_error(
                error_codes::ACTION_AGENT_SPAWN_FAILED,
                "act_spawn_agent observed the spawned session but failed to register the session process resource; exact spawned PID cleanup was attempted",
                json!({
                    "code": error_codes::ACTION_AGENT_SPAWN_FAILED,
                    "reason": "session_process_register_failed",
                    "spawn_id": spawn_id,
                    "cli": agent_kind.as_str(),
                    "launcher_process_id": launch_response.pid,
                    "agent_process_id": matched.agent_process_id,
                    "session_id": matched.session_id,
                    "log_dir": files.log_dir.display().to_string(),
                    "source_error": error.message,
                    "cleanup": cleanup,
                    "completion_artifacts": completion_artifacts,
                }),
            ));
        }

        // The bound MCP session now owns the credential; its teardown revokes it.
        spawn_credential.hand_off_to_session();
        Ok(ActSpawnAgentResponse {
            spawn_id,
            cli: agent_kind,
            kind: agent_kind,
            model_ref: local_model_row
                .as_ref()
                .map(|row| row.name.clone())
                .or_else(|| params.local_model_ref().map(ToOwned::to_owned)),
            launcher_process_id: launch_response.pid,
            agent_process_id: matched.agent_process_id,
            session_id: matched.session_id,
            mcp_url: params.mcp_url,
            working_dir: working_dir.display().to_string(),
            launch_target: launch_params.target,
            launch_target_source: launch_host.source,
            launched_at_unix_ms,
            registered_at_unix_ms: matched.registered_at_unix_ms,
            task_started_at_unix_ms: task_started.started_at_unix_ms,
            task_readiness_source: task_started.readiness_source.to_owned(),
            target: params.target,
            template_id: params.template_id,
            template_version: params.template_version,
            template_config_hash: params.template_config_hash,
            log_paths: files.to_response(),
        })
    }

    fn current_session_ids(&self) -> Result<BTreeSet<String>, ErrorData> {
        let guard = self.session_registry_ref().lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session registry lock poisoned while reading pre-spawn session ids",
            )
        })?;
        Ok(guard
            .reads(unix_time_ms_now())
            .into_iter()
            .map(|read| read.session_id)
            .collect())
    }

    fn spawn_session_candidates_for_readiness(
        &self,
        include_targets: bool,
    ) -> Result<Vec<SpawnSessionCandidateRead>, ErrorData> {
        let registry_reads = {
            let guard = self.session_registry_ref().lock().map_err(|_error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "session registry lock poisoned while reading spawn readiness candidates",
                )
            })?;
            guard.reads(unix_time_ms_now())
        };
        let mut candidates = Vec::with_capacity(registry_reads.len());
        for registry in registry_reads {
            let active_target = if include_targets {
                self.agent_logical_foreground_read_model(&registry.session_id)?
                    .map(|target| spawn_target_wire_from_session_target(&target))
            } else {
                None
            };
            candidates.push(SpawnSessionCandidateRead {
                registry,
                active_target,
            });
        }
        Ok(candidates)
    }

    async fn wait_for_spawned_agent_session(
        &self,
        params: &ActSpawnAgentParams,
        agent_kind: ActSpawnAgentCli,
        spawn_id: &str,
        before_session_ids: &BTreeSet<String>,
        launched_at_unix_ms: u64,
        launcher_pid: u32,
        files: &AgentSpawnFiles,
        deadline: Instant,
    ) -> Result<MatchedSpawnSession, serde_json::Value> {
        let mut last_observed = json!({
            "reason": "no_matching_session_observed",
            "sessions": [],
        });
        while !agent_spawn_deadline_remaining(deadline).is_zero() {
            if let Some(failure) = read_agent_spawn_terminal_failure(
                files,
                agent_kind,
                spawn_id,
                launched_at_unix_ms,
                launcher_pid,
            ) {
                return Err(failure);
            }
            let candidates = self
                .spawn_session_candidates_for_readiness(params.target.is_some())
                .map_err(|error| {
                    json!({
                        "reason": "spawn_readiness_candidate_read_failed",
                        "error": error.message,
                        "data": error.data,
                    })
                })?;
            let mut matched_session = None;
            let session_count = candidates.len();
            let mut sessions_json = Vec::new();
            let mut readiness_reason_counts: BTreeMap<String, u64> = BTreeMap::new();
            let mut candidate_readiness = Vec::new();
            let explicit_task_session_id = task_start_session_id_for_spawn(files, spawn_id);
            let mut explicit_task_session_match = None;
            let mut ready_candidates = Vec::new();
            // Sessions that are a new + in-window + CLI match for this spawn but
            // have not (yet) issued a daemon MCP tool call. They are identified
            // as ours; we bind one only with independent proof of task progress.
            let mut lenient_candidates: Vec<String> = Vec::new();
            for candidate in &candidates {
                let readiness = spawn_session_candidate_readiness_from_read(
                    &candidate.registry,
                    candidate.active_target.as_ref(),
                    agent_kind,
                    params.target.as_ref(),
                    before_session_ids,
                    launched_at_unix_ms,
                );
                let reason = readiness
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                *readiness_reason_counts.entry(reason.clone()).or_default() += 1;
                if reason == "tool_call_not_observed" {
                    lenient_candidates.push(candidate.registry.session_id.clone());
                }
                if explicit_task_session_id.as_deref()
                    == Some(candidate.registry.session_id.as_str())
                    && spawn_session_identity_matches_from_read(
                        &candidate.registry,
                        agent_kind,
                        before_session_ids,
                        launched_at_unix_ms,
                    )
                {
                    explicit_task_session_match = Some(MatchedSpawnSession {
                        session_id: candidate.registry.session_id.clone(),
                        registered_at_unix_ms: unix_time_ms_now(),
                        agent_process_id: discover_agent_process_id(launcher_pid, agent_kind),
                    });
                }
                if readiness.get("ready").and_then(Value::as_bool) == Some(true) {
                    ready_candidates.push(MatchedSpawnSession {
                        session_id: candidate.registry.session_id.clone(),
                        registered_at_unix_ms: unix_time_ms_now(),
                        agent_process_id: discover_agent_process_id(launcher_pid, agent_kind),
                    });
                }
                if reason != "session_existed_before_spawn"
                    && candidate_readiness.len() < AGENT_SPAWN_RECORDED_ATTEMPT_LIMIT
                {
                    candidate_readiness.push(json!({
                        "session_id": candidate.registry.session_id,
                        "started_at_unix_ms": candidate.registry.started_at_unix_ms,
                        "last_action": candidate.registry.last_action,
                        "active_target": candidate.active_target,
                        "readiness": readiness.clone(),
                    }));
                }
                if sessions_json.len() < AGENT_SPAWN_RECORDED_ATTEMPT_LIMIT {
                    sessions_json.push(spawn_session_observation_from_read(candidate, readiness));
                }
            }
            last_observed = json!({
                "reason": "candidate_not_ready",
                "session_count": session_count,
                "sessions_recorded": sessions_json.len(),
                "readiness_reason_counts": readiness_reason_counts,
                "candidate_readiness_recorded": candidate_readiness.len(),
                "candidate_readiness": candidate_readiness,
                "explicit_task_session_id": explicit_task_session_id.clone(),
                "ready_candidate_count": ready_candidates.len(),
                "sessions": sessions_json,
                "readiness_files": agent_spawn_readiness_file_readback(files),
                "read_model": "session_registry + per-session logical foreground target only; skips full session_list attached process/window scan",
            });

            if let Some(matched) = explicit_task_session_match {
                return Ok(matched);
            }
            if explicit_task_session_id.is_none() && ready_candidates.len() == 1 {
                matched_session = ready_candidates.pop();
            }
            if let Some(matched) = matched_session {
                return Ok(matched);
            }

            // Robust fallback for the agent-cooperative readiness protocol: a
            // session that registered, matches this spawn's CLI, and started in
            // the launch window is ours even if it never issued a daemon MCP
            // tool call (codex drives its own app-server tools; any agent may
            // skip the injected ceremony). Bind it only when the daemon
            // INDEPENDENTLY observes the task is underway, and only when the
            // candidate is unambiguous — fan_out disambiguates via the agent's
            // self-named session in the task-start artifact.
            if !lenient_candidates.is_empty()
                && agent_spawn_observed_task_progress(files, agent_kind).is_some()
            {
                let bind = if lenient_candidates.len() == 1 {
                    Some(lenient_candidates[0].clone())
                } else {
                    read_json_file_lossy(&files.task_started_path)
                        .and_then(|value| {
                            value
                                .get("session_id")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .filter(|session_id| lenient_candidates.contains(session_id))
                };
                if let Some(session_id) = bind {
                    return Ok(MatchedSpawnSession {
                        session_id,
                        registered_at_unix_ms: unix_time_ms_now(),
                        agent_process_id: discover_agent_process_id(launcher_pid, agent_kind),
                    });
                }
            }

            if process_has_exited(launcher_pid) {
                // The wrapper may have published failure after the loop-top
                // read, immediately before exiting. Prefer that final evidence.
                if let Some(failure) = read_agent_spawn_terminal_failure(
                    files,
                    agent_kind,
                    spawn_id,
                    launched_at_unix_ms,
                    launcher_pid,
                ) {
                    return Err(failure);
                }
                return Err(json!({
                    "reason": "launcher_process_exited_before_registry_match",
                    "launcher_process_id": launcher_pid,
                    "readiness_files": agent_spawn_readiness_file_readback(files),
                    "stdout_tail": public_agent_log_tail(&files.stdout_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                    "stderr_tail": public_agent_log_tail(&files.stderr_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                    "final_message_tail": public_agent_log_tail(&files.final_message_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                    "last_observed": last_observed,
                }));
            }
            sleep_agent_spawn_poll(deadline).await;
        }
        Err(last_observed)
    }

    async fn wait_for_spawned_agent_task_started(
        &self,
        params: &ActSpawnAgentParams,
        agent_kind: ActSpawnAgentCli,
        spawn_id: &str,
        matched: &mut MatchedSpawnSession,
        before_session_ids: &BTreeSet<String>,
        launched_at_unix_ms: u64,
        launcher_pid: u32,
        files: &AgentSpawnFiles,
        deadline: Instant,
    ) -> Result<AgentSpawnTaskStartRead, serde_json::Value> {
        let mut last_observed = json!({
            "reason": "task_start_artifact_not_observed",
            "task_started_path": files.task_started_path.display().to_string(),
        });
        while !agent_spawn_deadline_remaining(deadline).is_zero() {
            if let Some(failure) = read_agent_spawn_terminal_failure(
                files,
                agent_kind,
                spawn_id,
                launched_at_unix_ms,
                launcher_pid,
            ) {
                return Err(failure);
            }
            if let Err(liveness_error) =
                self.require_spawned_agent_session_live(&matched.session_id, files, agent_kind)
            {
                match self.rebind_spawned_agent_session_for_task_start(
                    params,
                    agent_kind,
                    spawn_id,
                    before_session_ids,
                    launched_at_unix_ms,
                    launcher_pid,
                    files,
                    &liveness_error,
                )? {
                    Some(rebound) => {
                        *matched = rebound;
                    }
                    None => {
                        last_observed = json!({
                            "reason": "matched_session_not_live_waiting_for_replacement",
                            "matched_session_id": matched.session_id,
                            "task_started_path": files.task_started_path.display().to_string(),
                            "session_liveness_error": liveness_error,
                            "readiness_files": agent_spawn_readiness_file_readback(files),
                            "observed_task_progress": agent_spawn_observed_task_progress(files, agent_kind),
                        });
                        if process_has_exited(launcher_pid) {
                            if let Some(failure) = read_agent_spawn_terminal_failure(
                                files,
                                agent_kind,
                                spawn_id,
                                launched_at_unix_ms,
                                launcher_pid,
                            ) {
                                return Err(failure);
                            }
                            return Err(json!({
                                "reason": "launcher_process_exited_after_matched_session_closed",
                                "launcher_process_id": launcher_pid,
                                "task_started_path": files.task_started_path.display().to_string(),
                                "completion_status": read_json_file_lossy(&files.completion_status_path),
                                "stdout_tail": public_agent_log_tail(&files.stdout_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                                "stderr_tail": public_agent_log_tail(&files.stderr_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                                "final_message_tail": public_agent_log_tail(&files.final_message_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                                "last_observed": last_observed,
                            }));
                        }
                        sleep_agent_spawn_poll(deadline).await;
                        continue;
                    }
                }
            }
            match read_agent_spawn_task_start_artifact(
                files, params, agent_kind, spawn_id, matched,
            )? {
                Some(read) => return Ok(read),
                None => {
                    // Observed work is diagnostic evidence, not the readiness
                    // verdict. #1225 requires durable session liveness plus the
                    // task-start artifact so a torn-down session cannot later be
                    // journaled as spawn_ready.
                    let observed_task_progress =
                        agent_spawn_observed_task_progress(files, agent_kind);
                    last_observed = json!({
                        "reason": "task_start_artifact_not_observed",
                        "task_started_path": files.task_started_path.display().to_string(),
                        "completion_status": read_json_file_lossy(&files.completion_status_path),
                        "observed_task_progress": observed_task_progress,
                        "stdout_bytes": file_len(&files.stdout_path),
                        "stderr_bytes": file_len(&files.stderr_path),
                        "final_message_bytes": file_len(&files.final_message_path),
                    });
                }
            }

            if process_has_exited(launcher_pid) {
                if let Some(failure) = read_agent_spawn_terminal_failure(
                    files,
                    agent_kind,
                    spawn_id,
                    launched_at_unix_ms,
                    launcher_pid,
                ) {
                    return Err(failure);
                }
                return Err(json!({
                    "reason": "launcher_process_exited_before_task_start",
                    "launcher_process_id": launcher_pid,
                    "task_started_path": files.task_started_path.display().to_string(),
                    "completion_status": read_json_file_lossy(&files.completion_status_path),
                    "stdout_tail": public_agent_log_tail(&files.stdout_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                    "stderr_tail": public_agent_log_tail(&files.stderr_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                    "final_message_tail": public_agent_log_tail(&files.final_message_path, AGENT_SPAWN_LOG_TAIL_BYTES),
                    "last_observed": last_observed,
                }));
            }
            sleep_agent_spawn_poll(deadline).await;
        }
        Err(json!({
            "reason": "task_start_artifact_timeout",
            "task_started_path": files.task_started_path.display().to_string(),
            "last_observed": last_observed,
        }))
    }

    fn rebind_spawned_agent_session_for_task_start(
        &self,
        params: &ActSpawnAgentParams,
        agent_kind: ActSpawnAgentCli,
        spawn_id: &str,
        before_session_ids: &BTreeSet<String>,
        launched_at_unix_ms: u64,
        launcher_pid: u32,
        files: &AgentSpawnFiles,
        liveness_error: &serde_json::Value,
    ) -> Result<Option<MatchedSpawnSession>, serde_json::Value> {
        let candidates = self
            .spawn_session_candidates_for_readiness(params.target.is_some())
            .map_err(|error| {
                json!({
                    "reason": "spawn_rebind_candidate_read_failed",
                    "error": error.message,
                    "data": error.data,
                    "session_liveness_error": liveness_error,
                })
            })?;
        let explicit_task_session_id = task_start_session_id_for_spawn(files, spawn_id);
        let observed_task_progress = agent_spawn_observed_task_progress(files, agent_kind);
        let mut ready_candidates = Vec::new();
        let mut lenient_candidates = Vec::new();
        let mut candidate_readiness = Vec::new();
        for candidate in &candidates {
            let readiness = spawn_session_candidate_readiness_from_read(
                &candidate.registry,
                candidate.active_target.as_ref(),
                agent_kind,
                params.target.as_ref(),
                before_session_ids,
                launched_at_unix_ms,
            );
            let reason = readiness
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if candidate_readiness.len() < AGENT_SPAWN_RECORDED_ATTEMPT_LIMIT
                && reason != "session_existed_before_spawn"
            {
                candidate_readiness.push(json!({
                    "session_id": candidate.registry.session_id,
                    "started_at_unix_ms": candidate.registry.started_at_unix_ms,
                    "last_action": candidate.registry.last_action,
                    "active_target": candidate.active_target,
                    "readiness": readiness.clone(),
                }));
            }
            if explicit_task_session_id.as_deref() == Some(candidate.registry.session_id.as_str())
                && spawn_session_identity_matches_from_read(
                    &candidate.registry,
                    agent_kind,
                    before_session_ids,
                    launched_at_unix_ms,
                )
            {
                return Ok(Some(MatchedSpawnSession {
                    session_id: candidate.registry.session_id.clone(),
                    registered_at_unix_ms: unix_time_ms_now(),
                    agent_process_id: discover_agent_process_id(launcher_pid, agent_kind),
                }));
            }
            if readiness.get("ready").and_then(Value::as_bool) == Some(true) {
                ready_candidates.push(candidate.registry.session_id.clone());
            } else if reason == "tool_call_not_observed" && observed_task_progress.is_some() {
                lenient_candidates.push(candidate.registry.session_id.clone());
            }
        }

        let bind = if ready_candidates.len() == 1 {
            Some(ready_candidates[0].clone())
        } else if ready_candidates.is_empty() && lenient_candidates.len() == 1 {
            Some(lenient_candidates[0].clone())
        } else if ready_candidates.len() + lenient_candidates.len() > 1 {
            return Err(json!({
                "reason": "spawn_rebind_ambiguous",
                "explicit_task_session_id": explicit_task_session_id,
                "ready_candidate_count": ready_candidates.len(),
                "lenient_candidate_count": lenient_candidates.len(),
                "ready_candidates": ready_candidates,
                "lenient_candidates": lenient_candidates,
                "candidate_readiness": candidate_readiness,
                "session_liveness_error": liveness_error,
                "readiness_files": agent_spawn_readiness_file_readback(files),
                "observed_task_progress": observed_task_progress,
            }));
        } else {
            None
        };

        Ok(bind.map(|session_id| MatchedSpawnSession {
            session_id,
            registered_at_unix_ms: unix_time_ms_now(),
            agent_process_id: discover_agent_process_id(launcher_pid, agent_kind),
        }))
    }

    fn require_spawned_agent_session_live(
        &self,
        session_id: &str,
        files: &AgentSpawnFiles,
        agent_kind: ActSpawnAgentCli,
    ) -> Result<(), serde_json::Value> {
        let liveness = self.spawned_agent_session_liveness_readback(session_id)?;
        if liveness.get("lifecycle").and_then(Value::as_str) == Some("live") {
            return Ok(());
        }
        Err(json!({
            "reason": "spawned_session_not_live",
            "session_id": session_id,
            "session_liveness": liveness,
            "readiness_files": agent_spawn_readiness_file_readback(files),
            "observed_task_progress": agent_spawn_observed_task_progress(files, agent_kind),
        }))
    }

    fn spawned_agent_session_liveness_readback(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, serde_json::Value> {
        let reads = {
            let guard = self.session_registry_ref().lock().map_err(|_error| {
                json!({
                    "reason": "session_registry_lock_poisoned",
                    "session_id": session_id,
                })
            })?;
            guard.reads(unix_time_ms_now())
        };
        let Some(summary) = reads.iter().find(|read| read.session_id == session_id) else {
            return Ok(json!({
                "reason": "spawned_session_missing_from_registry",
                "session_id": session_id,
                "session_count": reads.len(),
            }));
        };
        Ok(json!({
            "session_id": summary.session_id,
            "lifecycle": summary.lifecycle,
            "closed_at_unix_ms": summary.closed_at_unix_ms,
            "last_seen_unix_ms": summary.last_seen_unix_ms,
            "last_seen_ms_ago": summary.last_seen_ms_ago,
            "last_action": summary.last_action,
            "last_reason_code": summary.last_reason_code,
        }))
    }

    fn record_spawned_agent_metadata(
        &self,
        session_id: &str,
        metadata: SpawnedAgentRead,
    ) -> Result<(), ErrorData> {
        let mut registry = self.session_registry_ref().lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session registry lock poisoned while recording spawned agent metadata",
            )
        })?;
        registry.record_spawned_agent(session_id, metadata, unix_time_ms_now());
        Ok(())
    }

    /// Journals `spawn_requested` (#897) before any spawn side effect; a
    /// spawn whose lifecycle cannot be journaled is refused up front.
    fn journal_spawn_requested(
        &self,
        spawn_id: &str,
        params: &ActSpawnAgentParams,
        started_by_session_id: Option<&str>,
    ) -> Result<(), ErrorData> {
        let agent_kind = params.effective_cli()?;
        let log_dir = agent_spawn_root_dir()?.join(spawn_id);
        let db = self.m3_storage()?;
        let mut record = synapse_core::AgentEventRecord::new(
            super::agent_events::unix_time_ns_now(),
            synapse_core::AgentEventKind::SpawnRequested,
        );
        record.spawn_id = Some(spawn_id.to_owned());
        record.attributes.operation_name = Some(synapse_core::GenAiOperationName::CreateAgent);
        record.attributes.agent_name = Some(agent_kind.as_str().to_owned());
        record.attributes.provider_name =
            super::agent_events::provider_for_agent_kind(agent_kind.as_str());
        // Prompt CONTENT stays out of the journal (OTel GenAI opt-in rule);
        // its length is enough for the dashboard to size the request.
        record.payload = json!({
            "started_by_session_id": started_by_session_id,
            "prompt_chars": params.prompt.as_deref().map(|prompt| prompt.chars().count()),
            "working_dir": params.working_dir,
            "mcp_url": params.mcp_url,
            "wait_timeout_ms": params.wait_timeout_ms,
            "hold_open_ms": params.hold_open_ms,
            "kind": agent_kind.as_str(),
            "model_ref": params.local_model_ref(),
            "log_dir": log_dir.display().to_string(),
            // Spawn-template provenance (#909) so the journal records which
            // template version drove the run, not just the rendered params.
            "template_id": params.template_id.as_deref(),
            "template_version": params.template_version,
            "template_config_hash": params.template_config_hash.as_deref(),
        });
        super::agent_events::record_agent_event(&db, &record)
            .map(|_readback| ())
            .map_err(|error| {
                super::agent_events::agent_event_tool_error(ACT_SPAWN_AGENT, &error, false)
            })
    }

    /// Journals `spawn_ready` (#897) once the spawned agent has a registered
    /// MCP session and a validated task-start artifact.
    fn journal_spawn_ready(&self, response: &ActSpawnAgentResponse) -> Result<(), ErrorData> {
        let db = self.m3_storage()?;
        let mut record = synapse_core::AgentEventRecord::new(
            super::agent_events::unix_time_ns_now(),
            synapse_core::AgentEventKind::SpawnReady,
        );
        record.spawn_id = Some(response.spawn_id.clone());
        record.session_id = Some(response.session_id.clone());
        record.state_to = Some("live".to_owned());
        record.attributes.operation_name = Some(synapse_core::GenAiOperationName::InvokeAgent);
        record.attributes.agent_name = Some(response.cli.as_str().to_owned());
        record.attributes.provider_name =
            super::agent_events::provider_for_agent_kind(response.cli.as_str());
        record.attributes.conversation_id = Some(response.session_id.clone());
        record.payload = json!({
            "launcher_process_id": response.launcher_process_id,
            "agent_process_id": response.agent_process_id,
            "launched_at_unix_ms": response.launched_at_unix_ms,
            "registered_at_unix_ms": response.registered_at_unix_ms,
            "task_started_at_unix_ms": response.task_started_at_unix_ms,
            "launch_target": response.launch_target,
            "log_dir": response.log_paths.log_dir,
        });
        super::agent_events::record_agent_event(&db, &record)
            .map(|_readback| ())
            .map_err(|error| {
                super::agent_events::agent_event_tool_error(ACT_SPAWN_AGENT, &error, true)
            })
    }

    /// Journals a terminal `exited` event (durable flush) for a failed spawn
    /// (#897). The original spawn error always wins the tool response; a
    /// journal failure here is logged (AGENT_EVENT_WRITE_FAILED) not raised.
    fn journal_spawn_failed(&self, spawn_id: &str, error: &ErrorData) {
        let db = match self.m3_storage() {
            Ok(db) => db,
            Err(storage_error) => {
                tracing::error!(
                    code = "AGENT_EVENT_WRITE_FAILED",
                    spawn_id,
                    detail = %storage_error.message,
                    "spawn-failure agent event skipped: storage unavailable"
                );
                return;
            }
        };
        let reason = error
            .data
            .as_ref()
            .and_then(|data| data.get("reason"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("agent_spawn_failed");
        let error_code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str);
        let mut record = synapse_core::AgentEventRecord::new(
            super::agent_events::unix_time_ns_now(),
            synapse_core::AgentEventKind::Exited,
        );
        record.spawn_id = Some(spawn_id.to_owned());
        record.reason_code = Some(reason.to_owned());
        record.end_state = Some(synapse_core::AgentEndState::Error);
        record.attributes.error_type = error_code.map(ToOwned::to_owned);
        record.payload = json!({
            "error_message": error.message.chars().take(512).collect::<String>(),
        });
        if let Err(journal_error) = super::agent_events::record_agent_event_durable(&db, &record) {
            tracing::error!(
                code = "AGENT_EVENT_WRITE_FAILED",
                spawn_id,
                reason,
                detail = %journal_error,
                "spawn-failure agent event could not be journaled"
            );
        }
    }

    pub(super) async fn require_spawn_local_model_row(
        &self,
        params: &ActSpawnAgentParams,
        probe_by_session: &str,
    ) -> Result<LocalModelRegistryRow, ErrorData> {
        let model_ref = params.local_model_ref().ok_or_else(|| {
            local_model_spawn_refusal(
                error_codes::TOOL_PARAMS_INVALID,
                "local_model_model_ref_missing",
                "act_spawn_agent local_model requires model_ref",
                json!({
                    "source_of_truth": "CF_KV prefix local_model_registry/v1/model/name_hex/",
                }),
            )
        })?;
        let rows = self.local_model_registry_snapshot()?;
        let mut row = rows
            .into_iter()
            .find(|row| row.name == model_ref)
            .ok_or_else(|| {
                local_model_spawn_refusal(
                    error_codes::MODEL_REGISTRY_NOT_FOUND,
                    "local_model_registry_row_missing",
                    "act_spawn_agent local_model refused because the requested model_ref is not registered",
                    json!({
                        "model_ref": model_ref,
                        "source_of_truth": "CF_KV prefix local_model_registry/v1/model/name_hex/",
                    }),
                )
            })?;
        validate_spawn_local_model_static_requirements(model_ref, &row)?;
        let probe = row.last_probe.as_ref().ok_or_else(|| {
            local_model_spawn_refusal(
                error_codes::MODEL_REGISTRY_UNPROBED,
                "local_model_registry_row_unprobed",
                "act_spawn_agent local_model refused because the registry row has no probe evidence",
                json!({
                    "model_ref": model_ref,
                    "row_key": row.row_key.clone(),
                    "last_probe": null,
                    "source_of_truth": "CF_KV prefix local_model_registry/v1/model/name_hex/",
                }),
            )
        })?;
        if !probe.healthy {
            return Err(local_model_unhealthy_refusal(model_ref, &row));
        }
        let probe_age_ms = unix_time_ms_now().saturating_sub(probe.observed_at_unix_ms);
        if probe_age_ms > LOCAL_MODEL_SPAWN_MAX_PROBE_AGE_MS {
            tracing::warn!(
                code = "LOCAL_MODEL_SPAWN_STALE_PROBE_REFRESH",
                model_ref,
                row_key = %row.row_key,
                probe_age_ms,
                max_probe_age_ms = LOCAL_MODEL_SPAWN_MAX_PROBE_AGE_MS,
                probe_by_session,
                "act_spawn_agent refreshing stale local-model probe before launch"
            );
            let db = self.m3_storage()?;
            let refresh = crate::m3::local_models::probe_local_model(
                &db,
                &LocalModelProbeParams {
                    name: model_ref.to_owned(),
                    timeout_ms: None,
                },
                probe_by_session,
            )
            .await?;
            row = refresh.row;
            validate_spawn_local_model_static_requirements(model_ref, &row)?;
            let Some(refreshed_probe) = row.last_probe.as_ref() else {
                return Err(local_model_spawn_refusal(
                    error_codes::MODEL_REGISTRY_UNPROBED,
                    "local_model_registry_row_unprobed_after_refresh",
                    "act_spawn_agent local_model probe refresh did not persist probe evidence",
                    json!({
                        "model_ref": model_ref,
                        "row_key": row.row_key.clone(),
                        "source_of_truth": "CF_KV prefix local_model_registry/v1/model/name_hex/",
                    }),
                ));
            };
            if !refreshed_probe.healthy {
                return Err(local_model_unhealthy_refusal(model_ref, &row));
            }
            let refreshed_age_ms =
                unix_time_ms_now().saturating_sub(refreshed_probe.observed_at_unix_ms);
            if refreshed_age_ms > LOCAL_MODEL_SPAWN_MAX_PROBE_AGE_MS {
                return Err(local_model_spawn_refusal(
                    error_codes::MODEL_REGISTRY_PROBE_STALE,
                    "local_model_registry_probe_stale_after_refresh",
                    "act_spawn_agent local_model refused because probe refresh did not produce fresh evidence",
                    json!({
                        "model_ref": model_ref,
                        "row_key": row.row_key.clone(),
                        "last_probe": row.last_probe.clone(),
                        "probe_age_ms": refreshed_age_ms,
                        "max_probe_age_ms": LOCAL_MODEL_SPAWN_MAX_PROBE_AGE_MS,
                        "source_of_truth": "CF_KV prefix local_model_registry/v1/model/name_hex/",
                    }),
                ));
            }
            tracing::info!(
                code = "LOCAL_MODEL_SPAWN_STALE_PROBE_REFRESHED",
                model_ref,
                row_key = %row.row_key,
                refreshed_age_ms,
                probe_by_session,
                "act_spawn_agent refreshed stale local-model probe and will launch with fresh evidence"
            );
        }
        Ok(row)
    }
}

fn validate_spawn_local_model_static_requirements(
    model_ref: &str,
    row: &LocalModelRegistryRow,
) -> Result<(), ErrorData> {
    if !row.enabled {
        return Err(local_model_spawn_refusal(
            error_codes::MODEL_REGISTRY_DISABLED,
            "local_model_registry_row_disabled",
            "act_spawn_agent local_model refused because the registry row is disabled",
            json!({
                "model_ref": model_ref,
                "row_key": row.row_key.clone(),
                "enabled": row.enabled,
                "last_probe": row.last_probe.clone(),
                "source_of_truth": "CF_KV prefix local_model_registry/v1/model/name_hex/",
            }),
        ));
    }
    if row.api_shape != LocalModelApiShape::OpenAiChatCompletions {
        return Err(local_model_spawn_refusal(
            error_codes::MODEL_TOOLS_UNSUPPORTED,
            "local_model_api_shape_unsupported",
            "act_spawn_agent local_model refused because the registry row API shape is unsupported",
            json!({
                "model_ref": model_ref,
                "row_key": row.row_key.clone(),
                "api_shape": row.api_shape,
                "supported_api_shape": "open_ai_chat_completions",
            }),
        ));
    }
    Ok(())
}

fn local_model_unhealthy_refusal(model_ref: &str, row: &LocalModelRegistryRow) -> ErrorData {
    let code = match row
        .last_probe
        .as_ref()
        .and_then(|probe| probe.error_code.as_deref())
    {
        Some(error_codes::MODEL_TOOLS_UNSUPPORTED) => error_codes::MODEL_TOOLS_UNSUPPORTED,
        Some(error_codes::MODEL_ENDPOINT_UNREACHABLE) | None => {
            error_codes::MODEL_ENDPOINT_UNREACHABLE
        }
        Some(_) => error_codes::MODEL_ENDPOINT_UNREACHABLE,
    };
    local_model_spawn_refusal(
        code,
        "local_model_registry_row_unhealthy",
        "act_spawn_agent local_model refused because the registry row's last probe is unhealthy",
        json!({
            "model_ref": model_ref,
            "row_key": row.row_key.clone(),
            "last_probe": row.last_probe.clone(),
            "source_of_truth": "CF_KV prefix local_model_registry/v1/model/name_hex/",
        }),
    )
}

#[derive(Clone, Debug)]
struct MatchedSpawnSession {
    session_id: String,
    registered_at_unix_ms: u64,
    agent_process_id: Option<u32>,
}

#[derive(Debug)]
struct AgentSpawnTaskStartRead {
    started_at_unix_ms: u64,
    /// How task-start readiness was proven. #1225 keeps this fail-closed:
    /// successful readiness requires the cooperative task-start artifact plus a
    /// live spawned MCP session at readback time.
    readiness_source: &'static str,
}

#[derive(Debug)]
struct AgentSpawnFiles {
    log_dir: PathBuf,
    prompt_path: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    final_message_path: PathBuf,
    completion_status_path: PathBuf,
    task_started_path: PathBuf,
    task_started_script_path: PathBuf,
    debug_path: Option<PathBuf>,
    mcp_config_path: Option<PathBuf>,
    /// Claude only: generated `--settings` file wiring the CLI's HTTP hooks
    /// to the daemon's `/agent-events` ingress (#899).
    hook_settings_path: Option<PathBuf>,
    /// Codex only: generated `notify` program POSTing turn-complete events
    /// to the same ingress (#899).
    notify_script_path: Option<PathBuf>,
    /// Codex only: generated app-server runner script for interruptible turns.
    codex_app_server_runner_path: Option<PathBuf>,
    /// Codex only: control artifact with endpoint/thread/turn ids.
    codex_app_server_control_path: Option<PathBuf>,
    /// Codex only: raw app-server JSON-RPC event stream.
    codex_app_server_events_path: Option<PathBuf>,
    codex_app_server_stdout_path: Option<PathBuf>,
    codex_app_server_stderr_path: Option<PathBuf>,
    /// Local-model only: marker file written by the #931 runner.
    local_model_runner_path: Option<PathBuf>,
}

impl AgentSpawnFiles {
    fn to_response(&self) -> ActSpawnAgentLogPaths {
        let terminal = agent_spawn_terminal_capture_artifacts(&self.log_dir);
        ActSpawnAgentLogPaths {
            log_dir: self.log_dir.display().to_string(),
            prompt_path: self.prompt_path.display().to_string(),
            stdout_path: self.stdout_path.display().to_string(),
            stderr_path: self.stderr_path.display().to_string(),
            final_message_path: self.final_message_path.display().to_string(),
            completion_status_path: self.completion_status_path.display().to_string(),
            task_started_path: self.task_started_path.display().to_string(),
            task_started_script_path: self.task_started_script_path.display().to_string(),
            terminal_asciicast_path: terminal.asciicast_path.display().to_string(),
            terminal_capture_status_path: terminal.status_path.display().to_string(),
            terminal_final_screen_path: terminal.final_screen_path.display().to_string(),
            terminal_input_audit_path: terminal.input_audit_path.display().to_string(),
            debug_path: self
                .debug_path
                .as_ref()
                .map(|path| path.display().to_string()),
            mcp_config_path: self
                .mcp_config_path
                .as_ref()
                .map(|path| path.display().to_string()),
            hook_settings_path: self
                .hook_settings_path
                .as_ref()
                .map(|path| path.display().to_string()),
            notify_script_path: self
                .notify_script_path
                .as_ref()
                .map(|path| path.display().to_string()),
            codex_app_server_runner_path: self
                .codex_app_server_runner_path
                .as_ref()
                .map(|path| path.display().to_string()),
            codex_app_server_control_path: self
                .codex_app_server_control_path
                .as_ref()
                .map(|path| path.display().to_string()),
            codex_app_server_events_path: self
                .codex_app_server_events_path
                .as_ref()
                .map(|path| path.display().to_string()),
            codex_app_server_stdout_path: self
                .codex_app_server_stdout_path
                .as_ref()
                .map(|path| path.display().to_string()),
            codex_app_server_stderr_path: self
                .codex_app_server_stderr_path
                .as_ref()
                .map(|path| path.display().to_string()),
            local_model_runner_path: self
                .local_model_runner_path
                .as_ref()
                .map(|path| path.display().to_string()),
        }
    }
}

fn agent_spawn_terminal_capture_artifacts(log_dir: &Path) -> CaptureArtifacts {
    CaptureArtifacts {
        asciicast_path: log_dir.join("terminal.cast"),
        status_path: log_dir.join("terminal-capture-status.json"),
        final_screen_path: log_dir.join("terminal-final-screen.txt"),
        input_audit_path: log_dir.join("terminal-input-audit.ndjson"),
    }
}

fn agent_spawn_request_details(
    params: &ActSpawnAgentParams,
    started_by_session_id: Option<&str>,
) -> serde_json::Value {
    let agent_kind = params
        .effective_cli()
        .map(ActSpawnAgentCli::as_str)
        .unwrap_or("invalid");
    json!({
        "cli": agent_kind,
        "kind": agent_kind,
        "model_ref": params.local_model_ref(),
        "target": params.target,
        "working_dir": params.working_dir,
        "mcp_url": params.mcp_url,
        "wait_timeout_ms": params.wait_timeout_ms,
        "hold_open_ms": params.hold_open_ms,
        "prompt_present": params.prompt.as_ref().is_some_and(|prompt| !prompt.is_empty()),
        "prompt_bytes": params.prompt.as_ref().map_or(0, String::len),
        "started_by_session_id": started_by_session_id,
        "required_foreground": false,
        "launch_target_resolution": "runtime_powershell_host_preflight",
        "launch_target_env_var": AGENT_SPAWN_SHELL_ENV_VAR,
        "windows_console_window_state": "hidden",
    })
}

#[derive(Clone, Debug)]
struct AgentSpawnLaunchHost {
    target: String,
    source: String,
    attempted: Vec<String>,
}

impl AgentSpawnLaunchHost {
    fn to_json(&self) -> Value {
        json!({
        "target": self.target,
        "source": self.source,
        "attempted": self.attempted,
        "env_var": AGENT_SPAWN_SHELL_ENV_VAR,
        })
    }
}

fn resolve_agent_spawn_powershell_host() -> Result<AgentSpawnLaunchHost, ErrorData> {
    if let Some(configured) = std::env::var_os(AGENT_SPAWN_SHELL_ENV_VAR) {
        let configured = configured.into_string().map_err(|_| {
            agent_spawn_shell_error(
                "agent_spawn_shell_env_not_unicode",
                "act_spawn_agent launcher shell preflight failed because SYNAPSE_AGENT_SPAWN_SHELL is not valid Unicode",
                json!({
                    "env_var": AGENT_SPAWN_SHELL_ENV_VAR,
                    "supported_shells": ["pwsh.exe", "powershell.exe"],
                }),
            )
        })?;
        let candidate = trim_configured_agent_spawn_shell(&configured);
        if candidate.is_empty() {
            return Err(agent_spawn_shell_error(
                "agent_spawn_shell_env_empty",
                "act_spawn_agent launcher shell preflight failed because SYNAPSE_AGENT_SPAWN_SHELL is empty",
                json!({
                    "env_var": AGENT_SPAWN_SHELL_ENV_VAR,
                    "configured_value": configured,
                    "supported_shells": ["pwsh.exe", "powershell.exe"],
                }),
            ));
        }
        ensure_supported_agent_spawn_shell(candidate)?;
        let mut attempted = Vec::new();
        if let Some(target) = resolve_agent_spawn_shell_candidate(candidate, &mut attempted) {
            return Ok(AgentSpawnLaunchHost {
                target,
                source: format!("env:{AGENT_SPAWN_SHELL_ENV_VAR}"),
                attempted,
            });
        }
        return Err(agent_spawn_shell_error(
            "agent_spawn_shell_env_target_missing",
            "act_spawn_agent launcher shell preflight failed because SYNAPSE_AGENT_SPAWN_SHELL did not resolve to an executable file",
            json!({
                "env_var": AGENT_SPAWN_SHELL_ENV_VAR,
                "configured_value": configured,
                "normalized_candidate": candidate,
                "attempted": attempted,
                "supported_shells": ["pwsh.exe", "powershell.exe"],
            }),
        ));
    }

    resolve_default_agent_spawn_powershell_host()
}

fn trim_configured_agent_spawn_shell(value: &str) -> &str {
    let trimmed = value.trim();
    if let Some(stripped) = trimmed
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
    {
        return stripped.trim();
    }
    if let Some(stripped) = trimmed
        .strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
    {
        return stripped.trim();
    }
    trimmed
}

#[cfg(windows)]
fn resolve_default_agent_spawn_powershell_host() -> Result<AgentSpawnLaunchHost, ErrorData> {
    let mut attempted = Vec::new();
    for (source, candidate) in AGENT_SPAWN_WINDOWS_SHELL_CANDIDATES {
        ensure_supported_agent_spawn_shell(candidate)?;
        if let Some(target) = resolve_agent_spawn_shell_candidate(candidate, &mut attempted) {
            return Ok(AgentSpawnLaunchHost {
                target,
                source: (*source).to_owned(),
                attempted,
            });
        }
    }

    Err(agent_spawn_shell_error(
        "agent_spawn_shell_not_found",
        "act_spawn_agent launcher shell preflight failed because no supported PowerShell host was found",
        json!({
            "env_var": AGENT_SPAWN_SHELL_ENV_VAR,
            "attempted": attempted,
            "supported_shells": ["pwsh.exe", "powershell.exe"],
            "setup_action": "Install PowerShell 7 or ensure Windows PowerShell exists at C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe, then restart the synapse-mcp daemon so the process environment is current.",
        }),
    ))
}

#[cfg(not(windows))]
fn resolve_default_agent_spawn_powershell_host() -> Result<AgentSpawnLaunchHost, ErrorData> {
    let mut attempted = Vec::new();
    let candidate = "pwsh";
    if let Some(target) = resolve_agent_spawn_shell_candidate(candidate, &mut attempted) {
        return Ok(AgentSpawnLaunchHost {
            target,
            source: "path:pwsh".to_owned(),
            attempted,
        });
    }

    Err(agent_spawn_shell_error(
        "agent_spawn_shell_not_found",
        "act_spawn_agent launcher shell preflight failed because no supported PowerShell host was found",
        json!({
            "env_var": AGENT_SPAWN_SHELL_ENV_VAR,
            "attempted": attempted,
            "supported_shells": ["pwsh"],
        }),
    ))
}

fn ensure_supported_agent_spawn_shell(candidate: &str) -> Result<(), ErrorData> {
    let name = Path::new(candidate)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(candidate)
        .to_ascii_lowercase();
    let supported = if cfg!(windows) {
        matches!(
            name.as_str(),
            "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe"
        )
    } else {
        matches!(name.as_str(), "pwsh")
    };
    if supported {
        return Ok(());
    }

    Err(agent_spawn_shell_error(
        "agent_spawn_shell_unsupported",
        "act_spawn_agent launcher shell preflight failed because the configured shell is not a supported PowerShell host",
        json!({
            "env_var": AGENT_SPAWN_SHELL_ENV_VAR,
            "candidate": candidate,
            "observed_file_name": name,
            "supported_shells": if cfg!(windows) {
                json!(["pwsh.exe", "powershell.exe"])
            } else {
                json!(["pwsh"])
            },
        }),
    ))
}

fn resolve_agent_spawn_shell_candidate(
    candidate: &str,
    attempted: &mut Vec<String>,
) -> Option<String> {
    let candidate_path = Path::new(candidate);
    if is_path_like_agent_spawn_shell(candidate) {
        record_agent_spawn_shell_attempt(attempted, candidate_path);
        return candidate_path
            .is_file()
            .then(|| display_agent_spawn_shell_path(candidate_path));
    }

    let names = agent_spawn_executable_names(candidate);
    if let Some(path_value) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path_value) {
            for name in &names {
                let path = directory.join(name);
                record_agent_spawn_shell_attempt(attempted, &path);
                if path.is_file() {
                    return Some(display_agent_spawn_shell_path(&path));
                }
            }
        }
    }
    None
}

fn is_path_like_agent_spawn_shell(candidate: &str) -> bool {
    let path = Path::new(candidate);
    path.is_absolute()
        || candidate.contains('\\')
        || candidate.contains('/')
        || candidate
            .as_bytes()
            .get(1)
            .is_some_and(|second| *second == b':')
}

fn agent_spawn_executable_names(candidate: &str) -> Vec<String> {
    if Path::new(candidate).extension().is_some() {
        return vec![candidate.to_owned()];
    }

    let mut names = vec![candidate.to_owned()];
    for extension in agent_spawn_path_extensions() {
        names.push(format!("{candidate}{extension}"));
    }
    names
}

#[cfg(windows)]
fn agent_spawn_path_extensions() -> Vec<String> {
    let mut extensions = std::env::var_os("PATHEXT")
        .and_then(|value| value.into_string().ok())
        .map(|value| {
            value
                .split(';')
                .filter_map(|extension| {
                    let extension = extension.trim();
                    (!extension.is_empty()).then(|| extension.to_owned())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            vec![
                ".COM".to_owned(),
                ".EXE".to_owned(),
                ".BAT".to_owned(),
                ".CMD".to_owned(),
            ]
        });
    if !extensions
        .iter()
        .any(|extension| extension.eq_ignore_ascii_case(".exe"))
    {
        extensions.push(".EXE".to_owned());
    }
    extensions
}

#[cfg(not(windows))]
fn agent_spawn_path_extensions() -> Vec<String> {
    Vec::new()
}

fn record_agent_spawn_shell_attempt(attempted: &mut Vec<String>, path: &Path) {
    if attempted.len() < AGENT_SPAWN_RECORDED_ATTEMPT_LIMIT {
        attempted.push(path.display().to_string());
    }
}

fn display_agent_spawn_shell_path(path: &Path) -> String {
    if path.is_absolute() {
        return path.display().to_string();
    }
    std::env::current_dir()
        .map(|current_dir| current_dir.join(path).display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn agent_spawn_shell_error(
    reason: &'static str,
    message: &'static str,
    detail: Value,
) -> ErrorData {
    let mut data = Map::new();
    data.insert("code".to_owned(), json!(error_codes::ACTION_TARGET_INVALID));
    data.insert("reason".to_owned(), json!(reason));
    data.insert("tool".to_owned(), json!(ACT_SPAWN_AGENT));
    data.insert("detail".to_owned(), detail);
    agent_spawn_tool_error(
        error_codes::ACTION_TARGET_INVALID,
        message,
        Value::Object(data),
    )
}

fn augment_agent_spawn_error_with_artifacts(
    mut error: ErrorData,
    files: &AgentSpawnFiles,
    params: &ActSpawnAgentParams,
    spawn_id: &str,
    failure_stage: &'static str,
    launch_target: Option<&str>,
    completion_artifacts: Value,
) -> ErrorData {
    let mut data = match error.data.take() {
        Some(Value::Object(data)) => data,
        Some(source_data) => {
            let mut data = Map::new();
            data.insert("source_error_data".to_owned(), source_data);
            data
        }
        None => Map::new(),
    };
    let agent_kind = params.effective_cli().ok();
    data.entry("code".to_owned())
        .or_insert_with(|| json!(error_codes::ACTION_AGENT_SPAWN_FAILED));
    data.entry("reason".to_owned())
        .or_insert_with(|| json!(failure_stage));
    data.insert("agent_spawn_failure_stage".to_owned(), json!(failure_stage));
    data.insert("spawn_id".to_owned(), json!(spawn_id));
    data.insert(
        "cli".to_owned(),
        json!(
            agent_kind
                .map(ActSpawnAgentCli::as_str)
                .unwrap_or("invalid")
        ),
    );
    data.insert("mcp_url".to_owned(), json!(params.mcp_url));
    data.insert(
        "log_dir".to_owned(),
        json!(files.log_dir.display().to_string()),
    );
    if let Some(launch_target) = launch_target {
        data.insert("launch_target".to_owned(), json!(launch_target));
    }
    data.insert("completion_artifacts".to_owned(), completion_artifacts);
    error.data = Some(Value::Object(data));
    error
}

fn write_agent_spawn_daemon_terminal_artifacts(
    files: &AgentSpawnFiles,
    params: &ActSpawnAgentParams,
    spawn_id: &str,
    status: &str,
    error_message: &str,
    details: serde_json::Value,
) -> serde_json::Value {
    let agent_kind = params.effective_cli().ok();
    let agent_kind = agent_kind
        .map(ActSpawnAgentCli::as_str)
        .unwrap_or("invalid");
    let completed_at_unix_ms = unix_time_ms_now();
    let stdout_len = file_len(&files.stdout_path);
    let stderr_len = file_len(&files.stderr_path);
    let terminal = agent_spawn_terminal_capture_artifacts(&files.log_dir);
    let final_message = json!({
        "schema_version": 1,
        "spawn_id": spawn_id,
        "cli": agent_kind,
        "kind": agent_kind,
        "status": status,
        "exit_code": null,
        "error_message": error_message,
        "message": "Synapse act_spawn_agent wrote this terminal artifact because the daemon ended the spawn before a final assistant response was available.",
        "stdout_path": files.stdout_path.display().to_string(),
        "stderr_path": files.stderr_path.display().to_string(),
        "completion_status_path": files.completion_status_path.display().to_string(),
        "task_started_path": files.task_started_path.display().to_string(),
        "terminal_asciicast_path": terminal.asciicast_path.display().to_string(),
        "terminal_capture_status_path": terminal.status_path.display().to_string(),
        "terminal_final_screen_path": terminal.final_screen_path.display().to_string(),
        "terminal_input_audit_path": terminal.input_audit_path.display().to_string(),
        "details": details,
    });
    let final_write = serde_json::to_vec_pretty(&final_message)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            fs::write(&files.final_message_path, bytes).map_err(|error| error.to_string())
        });
    let final_len = file_len(&files.final_message_path);
    let completion_status = json!({
        "schema_version": 1,
        "spawn_id": spawn_id,
        "cli": agent_kind,
        "kind": agent_kind,
        "status": status,
        "exit_code": null,
        "error_message": error_message,
        "wrapper_started_at_unix_ms": null,
        "completed_at_unix_ms": completed_at_unix_ms,
        "elapsed_ms": null,
        "requested_hold_open_ms": params.hold_open_ms,
        "hold_open_elapsed_ms_met": false,
        "final_message_path": files.final_message_path.display().to_string(),
        "final_message_bytes": final_len,
        "final_message_present": final_len > 0,
        "final_message_source": "daemon_terminal_artifact_json",
        "recovered_final_message_written": false,
        "fallback_final_message_written": true,
        "stdout_path": files.stdout_path.display().to_string(),
        "stdout_line_count": null,
        "last_stdout_event_type": null,
        "stdout_bytes": stdout_len,
        "stderr_path": files.stderr_path.display().to_string(),
        "stderr_bytes": stderr_len,
        "terminal_asciicast_path": terminal.asciicast_path.display().to_string(),
        "terminal_asciicast_bytes": file_len(&terminal.asciicast_path),
        "terminal_capture_status_path": terminal.status_path.display().to_string(),
        "terminal_capture_status_bytes": file_len(&terminal.status_path),
        "terminal_final_screen_path": terminal.final_screen_path.display().to_string(),
        "terminal_final_screen_bytes": file_len(&terminal.final_screen_path),
        "terminal_input_audit_path": terminal.input_audit_path.display().to_string(),
        "terminal_input_audit_bytes": file_len(&terminal.input_audit_path),
        "daemon_terminal_artifact": true,
    });
    let status_write = serde_json::to_vec_pretty(&completion_status)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            fs::write(&files.completion_status_path, bytes).map_err(|error| error.to_string())
        });
    let status_len = file_len(&files.completion_status_path);
    json!({
        "final_message_path": files.final_message_path.display().to_string(),
        "final_message_write_ok": final_write.is_ok(),
        "final_message_write_error": final_write.err(),
        "final_message_bytes_after": final_len,
        "completion_status_path": files.completion_status_path.display().to_string(),
        "completion_status_write_ok": status_write.is_ok(),
        "completion_status_write_error": status_write.err(),
        "completion_status_bytes_after": status_len,
        "task_started_path": files.task_started_path.display().to_string(),
        "task_started_bytes": file_len(&files.task_started_path),
    })
}

/// Resolve the API-key credential that a spawned local-model agent must carry
/// into its child process environment.
///
/// `act_launch` clears the child environment, so the daemon is the only place
/// the credential lives at spawn time. Returns:
/// - `Ok(None)` when the row needs no key (loopback model, or non-local-model
///   spawn),
/// - `Ok(Some((env_var, value)))` when the daemon has the declared key set to a
///   non-empty value, ready to inject into the child env,
/// - `Err(..)` (MODEL_API_KEY_MISSING) when the row declares an
///   `api_key_env_var` the daemon does not have, so the spawn is refused loudly
///   instead of launching an agent that will 401 on its first model call.
fn resolve_spawn_local_model_api_key(
    db: &std::sync::Arc<synapse_storage::Db>,
    local_model_row: Option<&LocalModelRegistryRow>,
) -> Result<Option<(String, String)>, ErrorData> {
    let Some(row) = local_model_row else {
        return Ok(None);
    };
    // Single resolution point shared with probing: encrypted DPAPI secret store
    // first, then the daemon process environment, else a loud refusal. Only the
    // resolved value is forwarded into the child env (never persisted).
    match crate::m3::local_models::resolve_local_model_api_key(db, row) {
        Ok(ResolvedApiKey::NotRequired) => Ok(None),
        Ok(ResolvedApiKey::Resolved {
            env_var,
            value,
            source,
        }) => {
            // Audit which credential source authenticated the spawn (never the
            // value). "dpapi_secret_store" means the encrypted at-rest key was
            // used; "process_env" means the daemon environment fallback.
            tracing::info!(
                model = %row.name,
                env_var = %env_var,
                source,
                "resolved local-model API key for spawn"
            );
            Ok(Some((env_var, value)))
        }
        Err(error) => {
            let code = error
                .data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(serde_json::Value::as_str);
            let (refusal_code, reason): (&'static str, &'static str) = match code {
                Some(error_codes::MODEL_API_KEY_DECRYPT_FAILED) => (
                    error_codes::MODEL_API_KEY_DECRYPT_FAILED,
                    "local_model_api_key_decrypt_failed",
                ),
                _ => (
                    error_codes::MODEL_API_KEY_MISSING,
                    "local_model_api_key_missing",
                ),
            };
            Err(local_model_spawn_refusal(
                refusal_code,
                reason,
                "act_spawn_agent local_model refused because no API key could be resolved for the model from the encrypted secret store or the daemon environment",
                json!({
                    "model_ref": row.name,
                    "api_key_env_var": row.api_key_env_var,
                    "resolver_error": error.message,
                    "resolver_data": error.data,
                    "remediation": "store the key on this Windows account via the dashboard Add/Edit API Model form or local_model_update { name, api_key }, or set the environment variable before launching the daemon",
                    "source_of_truth": "DPAPI secret store (CF_KV local_model_secret/v1) + daemon process environment",
                }),
            ))
        }
    }
}

fn local_model_spawn_refusal(
    code: &'static str,
    reason: &'static str,
    message: &'static str,
    detail: Value,
) -> ErrorData {
    agent_spawn_tool_error(
        code,
        message,
        json!({
            "code": code,
            "reason": reason,
            "tool": ACT_SPAWN_AGENT,
            "detail": detail,
        }),
    )
}

#[derive(Debug, Default)]
struct AgentSpawnOrphanRecoveryReport {
    scanned_count: usize,
    recovered_count: usize,
    skipped_terminal_count: usize,
    skipped_live_count: usize,
    skipped_fresh_count: usize,
    recovered_spawn_ids: Vec<String>,
}

fn recover_orphaned_agent_spawn_terminal_artifacts()
-> Result<AgentSpawnOrphanRecoveryReport, ErrorData> {
    let root = agent_spawn_root_dir()?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AgentSpawnOrphanRecoveryReport::default());
        }
        Err(error) => {
            return Err(mcp_error(
                error_codes::STORAGE_READ_FAILED,
                format!(
                    "act_spawn_agent failed to read agent spawn root {} during orphan recovery: {error}",
                    root.display()
                ),
            ));
        }
    };
    let now = unix_time_ms_now();
    let mut report = AgentSpawnOrphanRecoveryReport::default();
    for entry in entries {
        let entry = entry.map_err(|error| {
            mcp_error(
                error_codes::STORAGE_READ_FAILED,
                format!(
                    "act_spawn_agent failed to read an agent spawn root entry at {} during orphan recovery: {error}",
                    root.display()
                ),
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            mcp_error(
                error_codes::STORAGE_READ_FAILED,
                format!(
                    "act_spawn_agent failed to read file type for {} during orphan recovery: {error}",
                    entry.path().display()
                ),
            )
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let spawn_id = entry.file_name().to_string_lossy().into_owned();
        if !spawn_id.starts_with("agent-spawn-") {
            continue;
        }
        report.scanned_count += 1;
        let log_dir = entry.path();
        let decision = agent_spawn_orphan_recovery_decision(&spawn_id, &log_dir, now)?;
        match decision {
            AgentSpawnOrphanRecoveryDecision::SkipTerminal => {
                report.skipped_terminal_count += 1;
            }
            AgentSpawnOrphanRecoveryDecision::SkipLive => {
                report.skipped_live_count += 1;
            }
            AgentSpawnOrphanRecoveryDecision::SkipFresh => {
                report.skipped_fresh_count += 1;
            }
            AgentSpawnOrphanRecoveryDecision::Recover(recovery) => {
                write_agent_spawn_orphan_terminal_artifacts(&spawn_id, &log_dir, &recovery)?;
                report.recovered_count += 1;
                report.recovered_spawn_ids.push(spawn_id);
            }
        }
    }
    Ok(report)
}

#[derive(Debug)]
enum AgentSpawnOrphanRecoveryDecision {
    SkipTerminal,
    SkipLive,
    SkipFresh,
    Recover(AgentSpawnOrphanRecovery),
}

#[derive(Debug)]
struct AgentSpawnOrphanRecovery {
    status: &'static str,
    reason: &'static str,
    cli: String,
    wrapper_process_id: Option<u32>,
    source_completion_status: Option<Value>,
    source_completion_status_error: Option<String>,
    status_age_ms: Option<u64>,
}

fn agent_spawn_orphan_recovery_decision(
    spawn_id: &str,
    log_dir: &Path,
    now: u64,
) -> Result<AgentSpawnOrphanRecoveryDecision, ErrorData> {
    let completion_status_path = log_dir.join("completion-status.json");
    let status_age_ms =
        file_age_ms(&completion_status_path, now).or_else(|| file_age_ms(log_dir, now));
    let status_bytes = match fs::read(&completion_status_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if stale_enough_for_orphan_recovery(status_age_ms) {
                return Ok(AgentSpawnOrphanRecoveryDecision::Recover(
                    AgentSpawnOrphanRecovery {
                        status: "orphaned_status_missing_recovered",
                        reason: "missing_completion_status_stale",
                        cli: "unknown".to_owned(),
                        wrapper_process_id: None,
                        source_completion_status: None,
                        source_completion_status_error: Some(format!(
                            "completion-status.json was missing for stale spawn directory {spawn_id}"
                        )),
                        status_age_ms,
                    },
                ));
            }
            return Ok(AgentSpawnOrphanRecoveryDecision::SkipFresh);
        }
        Err(error) => {
            return Err(mcp_error(
                error_codes::STORAGE_READ_FAILED,
                format!(
                    "act_spawn_agent failed to read agent spawn completion status {} during orphan recovery: {error}",
                    completion_status_path.display()
                ),
            ));
        }
    };
    let status_json = match serde_json::from_slice::<Value>(&status_bytes) {
        Ok(status_json) => status_json,
        Err(error) => {
            if stale_enough_for_orphan_recovery(status_age_ms) {
                return Ok(AgentSpawnOrphanRecoveryDecision::Recover(
                    AgentSpawnOrphanRecovery {
                        status: "orphaned_status_invalid_recovered",
                        reason: "invalid_completion_status_stale",
                        cli: "unknown".to_owned(),
                        wrapper_process_id: None,
                        source_completion_status: None,
                        source_completion_status_error: Some(error.to_string()),
                        status_age_ms,
                    },
                ));
            }
            return Ok(AgentSpawnOrphanRecoveryDecision::SkipFresh);
        }
    };
    let Some(status) = status_json.get("status").and_then(Value::as_str) else {
        if stale_enough_for_orphan_recovery(status_age_ms) {
            return Ok(AgentSpawnOrphanRecoveryDecision::Recover(
                AgentSpawnOrphanRecovery {
                    status: "orphaned_status_invalid_recovered",
                    reason: "missing_status_field_stale",
                    cli: status_json
                        .get("cli")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned(),
                    wrapper_process_id: wrapper_process_id_from_status(&status_json),
                    source_completion_status: Some(status_json),
                    source_completion_status_error: None,
                    status_age_ms,
                },
            ));
        }
        return Ok(AgentSpawnOrphanRecoveryDecision::SkipFresh);
    };
    if status != "running" {
        return Ok(AgentSpawnOrphanRecoveryDecision::SkipTerminal);
    }
    let wrapper_process_id = wrapper_process_id_from_status(&status_json);
    if let Some(pid) = wrapper_process_id {
        if wrapper_process_is_live_for_status(pid, &status_json) {
            return Ok(AgentSpawnOrphanRecoveryDecision::SkipLive);
        }
        return Ok(AgentSpawnOrphanRecoveryDecision::Recover(
            AgentSpawnOrphanRecovery {
                status: "orphaned_running_recovered",
                reason: "running_status_wrapper_process_gone",
                cli: status_json
                    .get("cli")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                wrapper_process_id,
                source_completion_status: Some(status_json),
                source_completion_status_error: None,
                status_age_ms,
            },
        ));
    }
    if stale_enough_for_orphan_recovery(status_age_ms) {
        return Ok(AgentSpawnOrphanRecoveryDecision::Recover(
            AgentSpawnOrphanRecovery {
                status: "orphaned_running_recovered",
                reason: "running_status_without_wrapper_pid_stale",
                cli: status_json
                    .get("cli")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                wrapper_process_id: None,
                source_completion_status: Some(status_json),
                source_completion_status_error: None,
                status_age_ms,
            },
        ));
    }
    Ok(AgentSpawnOrphanRecoveryDecision::SkipFresh)
}

fn stale_enough_for_orphan_recovery(age_ms: Option<u64>) -> bool {
    age_ms.is_some_and(|age_ms| age_ms >= AGENT_SPAWN_ORPHAN_RECOVERY_STALE_MS)
}

fn wrapper_process_id_from_status(status: &Value) -> Option<u32> {
    status
        .get("wrapper_process_id")
        .or_else(|| status.get("powershell_process_id"))
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
}

fn wrapper_process_is_live_for_status(pid: u32, status: &Value) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let mut system = System::new();
    let sys_pid = Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[sys_pid]), false);
    let Some(process) = system.process(sys_pid) else {
        return false;
    };
    let Some(wrapper_started_at_unix_ms) = status
        .get("wrapper_started_at_unix_ms")
        .and_then(Value::as_u64)
    else {
        return true;
    };
    let process_started_at_unix_ms = process.start_time().saturating_mul(1000);
    process_started_at_unix_ms.abs_diff(wrapper_started_at_unix_ms) <= 30_000
}

fn wrapper_process_is_live_for_recovery(recovery: &AgentSpawnOrphanRecovery) -> bool {
    let Some(pid) = recovery.wrapper_process_id else {
        return false;
    };
    if let Some(status) = &recovery.source_completion_status {
        wrapper_process_is_live_for_status(pid, status)
    } else {
        process_exists(pid)
    }
}

fn write_agent_spawn_orphan_terminal_artifacts(
    spawn_id: &str,
    log_dir: &Path,
    recovery: &AgentSpawnOrphanRecovery,
) -> Result<(), ErrorData> {
    let stdout_path = log_dir.join("stdout.jsonl");
    let stderr_path = log_dir.join("stderr.log");
    let final_message_path = log_dir.join("final-message.txt");
    let completion_status_path = log_dir.join("completion-status.json");
    let stdout_len = file_len(&stdout_path);
    let stderr_len = file_len(&stderr_path);
    let final_message_len_before = file_len(&final_message_path);
    let (stdout_line_count, last_stdout_event_type) = stdout_summary_lossy(&stdout_path);
    let stdout_tail = public_agent_log_tail(&stdout_path, AGENT_SPAWN_LOG_TAIL_BYTES);
    let stderr_tail = public_agent_log_tail(&stderr_path, AGENT_SPAWN_LOG_TAIL_BYTES);
    let completed_at_unix_ms = unix_time_ms_now();
    let details = json!({
        "reason": recovery.reason,
        "spawn_id": spawn_id,
        "log_dir": log_dir.display().to_string(),
        "wrapper_process_id": recovery.wrapper_process_id,
        "wrapper_process_live": wrapper_process_is_live_for_recovery(recovery),
        "status_age_ms": recovery.status_age_ms,
        "stale_threshold_ms": AGENT_SPAWN_ORPHAN_RECOVERY_STALE_MS,
        "source_completion_status": &recovery.source_completion_status,
        "source_completion_status_error": &recovery.source_completion_status_error,
        "stdout_tail": stdout_tail,
        "stderr_tail": stderr_tail,
    });
    if final_message_len_before == 0 {
        let final_message = json!({
            "schema_version": 1,
            "spawn_id": spawn_id,
            "cli": &recovery.cli,
            "status": recovery.status,
            "exit_code": null,
            "error_message": "agent spawn wrapper exited, disappeared, or daemon state was lost before a terminal completion artifact was written",
            "message": "Synapse act_spawn_agent orphan recovery wrote this terminal artifact because a stale spawn directory did not contain a terminal final-message/completion-status pair.",
            "stdout_path": stdout_path.display().to_string(),
            "stderr_path": stderr_path.display().to_string(),
            "completion_status_path": completion_status_path.display().to_string(),
            "details": details,
        });
        let bytes = serde_json::to_vec_pretty(&final_message).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("act_spawn_agent failed to encode orphan recovery final-message artifact: {error}"),
            )
        })?;
        fs::write(&final_message_path, bytes).map_err(|error| {
            mcp_error(
                error_codes::STORAGE_WRITE_FAILED,
                format!(
                    "act_spawn_agent failed to write orphan recovery final-message artifact {}: {error}",
                    final_message_path.display()
                ),
            )
        })?;
    }
    let final_message_len_after = file_len(&final_message_path);
    let completion_status = json!({
        "schema_version": 1,
        "spawn_id": spawn_id,
        "cli": &recovery.cli,
        "status": recovery.status,
        "exit_code": null,
        "error_message": "agent spawn wrapper exited, disappeared, or daemon state was lost before a terminal completion artifact was written",
        "wrapper_process_id": recovery.wrapper_process_id,
        "wrapper_process_live": wrapper_process_is_live_for_recovery(recovery),
        "wrapper_started_at_unix_ms": recovery
            .source_completion_status
            .as_ref()
            .and_then(|status| status.get("wrapper_started_at_unix_ms"))
            .and_then(Value::as_u64),
        "completed_at_unix_ms": completed_at_unix_ms,
        "elapsed_ms": null,
        "requested_hold_open_ms": recovery
            .source_completion_status
            .as_ref()
            .and_then(|status| status.get("requested_hold_open_ms"))
            .cloned(),
        "hold_open_elapsed_ms_met": false,
        "final_message_path": final_message_path.display().to_string(),
        "final_message_bytes": final_message_len_after,
        "final_message_present": final_message_len_after > 0,
        "final_message_source": if final_message_len_before > 0 {
            "preexisting_final_message_without_terminal_status"
        } else {
            "orphan_recovery_artifact_json"
        },
        "recovered_final_message_written": false,
        "fallback_final_message_written": final_message_len_before == 0,
        "stdout_path": stdout_path.display().to_string(),
        "stdout_line_count": stdout_line_count,
        "last_stdout_event_type": last_stdout_event_type,
        "stdout_bytes": stdout_len,
        "stderr_path": stderr_path.display().to_string(),
        "stderr_bytes": stderr_len,
        "daemon_terminal_artifact": true,
        "orphan_recovery_artifact": true,
        "completion_status_source": "act_spawn_agent_orphan_recovery",
        "source_completion_status_error": &recovery.source_completion_status_error,
        "log_dir": log_dir.display().to_string(),
        "details": details,
    });
    let bytes = serde_json::to_vec_pretty(&completion_status).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("act_spawn_agent failed to encode orphan recovery completion-status artifact: {error}"),
        )
    })?;
    fs::write(&completion_status_path, bytes).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            format!(
                "act_spawn_agent failed to write orphan recovery completion-status artifact {}: {error}",
                completion_status_path.display()
            ),
        )
    })?;
    Ok(())
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

fn file_age_ms(path: &Path, now: u64) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let modified = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
    let modified = u64::try_from(modified).ok()?;
    Some(now.saturating_sub(modified))
}

fn stdout_summary_lossy(path: &Path) -> (u64, Option<String>) {
    let Some(stdout) = tail_file_lossy(path, usize::MAX) else {
        return (0, None);
    };
    let mut line_count = 0;
    let mut last_event_type = None;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        line_count += 1;
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if let Some(event_type) = value.get("type").and_then(Value::as_str) {
                last_event_type = Some(event_type.to_owned());
            } else if let Some(event_type) = value
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
            {
                last_event_type = Some(event_type.to_owned());
            }
        }
    }
    (line_count, last_event_type)
}

pub(super) fn validate_cli_model_before_spawn(
    params: &ActSpawnAgentParams,
    agent_kind: ActSpawnAgentCli,
) -> Result<(), ErrorData> {
    let Some(model) = params.model.as_deref() else {
        return Ok(());
    };
    match agent_kind {
        ActSpawnAgentCli::Codex => validate_codex_model_before_spawn(model),
        ActSpawnAgentCli::Claude => {
            validate_claude_model_before_spawn(model, params.working_dir.as_deref())
        }
        ActSpawnAgentCli::LocalModel => Ok(()),
    }
}

fn validate_codex_model_before_spawn(model: &str) -> Result<(), ErrorData> {
    // Codex's built-in openai provider honors OPENAI_BASE_URL (forwarded to the
    // spawned agent by the child-environment policy).
    if is_custom_openai_base_url(std::env::var("OPENAI_BASE_URL").ok().as_deref()) {
        return Ok(());
    }
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "act_spawn_agent cannot validate the Codex model because CODEX_HOME/USERPROFILE/HOME is unavailable",
            )
        })?;
    validate_codex_model_at_home(model, &codex_home)
}

fn validate_codex_model_at_home(model: &str, codex_home: &Path) -> Result<(), ErrorData> {
    let config = fs::read_to_string(codex_home.join("config.toml")).unwrap_or_default();
    if codex_model_config_allows(model, &config)? {
        return Ok(());
    }
    let cache = fs::read_to_string(codex_home.join("models_cache.json")).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "act_spawn_agent cannot validate Codex model {model:?}: {} is unavailable ({error}); open Codex once to refresh its model catalog or omit model to use the configured default",
                codex_home.join("models_cache.json").display()
            ),
        )
    })?;
    validate_codex_model_with_sources(model, &config, &cache)
}

fn validate_codex_model_with_sources(
    model: &str,
    config: &str,
    cache: &str,
) -> Result<(), ErrorData> {
    if codex_model_config_allows(model, config)? {
        return Ok(());
    }
    let catalog: Value = serde_json::from_str(cache).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_spawn_agent cannot validate Codex model {model:?}: models_cache.json is malformed ({error})"),
        )
    })?;
    let known = catalog
        .get("models")
        .and_then(Value::as_array)
        .is_some_and(|models| {
            models
                .iter()
                .any(|entry| entry.get("slug").and_then(Value::as_str) == Some(model))
        });
    if known {
        return Ok(());
    }
    Err(mcp_error(
        error_codes::TOOL_PARAMS_INVALID,
        format!(
            "act_spawn_agent refused unknown Codex model {model:?} before creating spawn artifacts; use a model in Codex's local catalog, configure it as the Codex default, select a custom model_provider, or omit model"
        ),
    ))
}

fn codex_model_config_allows(model: &str, config: &str) -> Result<bool, ErrorData> {
    if config.trim().is_empty() {
        return Ok(false);
    }
    let config: toml::Value = toml::from_str(config).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_spawn_agent cannot validate Codex model {model:?}: config.toml is malformed ({error})"),
        )
    })?;
    let active_profile = config.get("profile").and_then(toml::Value::as_str);
    let profile = active_profile.and_then(|name| {
        config
            .get("profiles")
            .and_then(|profiles| profiles.get(name))
    });
    let configured_model = profile
        .and_then(|profile| profile.get("model"))
        .and_then(toml::Value::as_str)
        .or_else(|| config.get("model").and_then(toml::Value::as_str));
    if configured_model == Some(model) {
        return Ok(true);
    }
    let provider = profile
        .and_then(|profile| profile.get("model_provider"))
        .and_then(toml::Value::as_str)
        .or_else(|| config.get("model_provider").and_then(toml::Value::as_str))
        .unwrap_or("openai");
    if !provider.eq_ignore_ascii_case("openai") {
        return Ok(true);
    }
    let base_url = config
        .get("model_providers")
        .and_then(|providers| providers.get(provider))
        .and_then(|provider| provider.get("base_url"))
        .and_then(toml::Value::as_str);
    Ok(is_custom_openai_base_url(base_url))
}

/// True when `url` points the built-in OpenAI provider at a gateway instead of
/// api.openai.com. Gateways own their model namespace, so the built-in model
/// allowlist does not apply and the CLI reports unknown ids itself.
fn is_custom_openai_base_url(url: Option<&str>) -> bool {
    url.map(str::trim)
        .is_some_and(|url| !url.is_empty() && !url.to_ascii_lowercase().contains("api.openai.com"))
}

fn validate_claude_model_before_spawn(
    model: &str,
    working_dir: Option<&str>,
) -> Result<(), ErrorData> {
    validate_claude_model_before_spawn_with(model, working_dir, &process_env_var)
}

/// The live process-environment lookup passed to the `_with` seams.
fn process_env_var(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name)
}

/// Test seam for [`validate_claude_model_before_spawn`]: `env` replaces the
/// process environment for both the `ANTHROPIC_*`/`CLAUDE_CODE_*` reads and
/// the settings roots (`USERPROFILE`/`HOME`, `ProgramFiles`).
fn validate_claude_model_before_spawn_with(
    model: &str,
    working_dir: Option<&str>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<(), ErrorData> {
    let mut configured_env = BTreeMap::new();
    let mut configured_models = BTreeSet::new();
    for name in [
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
    ] {
        if let Some(value) = env(name).and_then(|value| value.into_string().ok()) {
            configured_env.insert(name.to_owned(), value);
        }
    }
    let settings_paths = claude_settings_paths(working_dir, env);
    for path in settings_paths {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        merge_claude_model_settings(&settings, &mut configured_env, &mut configured_models);
    }
    let custom_provider = claude_custom_provider(&configured_env);
    [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
    ]
    .iter()
    .filter_map(|name| configured_env.get(*name).cloned())
    .for_each(|model| {
        configured_models.insert(model);
    });
    let configured_models = configured_models.into_iter().collect::<Vec<_>>();
    validate_claude_model_with_context(model, custom_provider, &configured_models)
}

/// Claude Code settings files in precedence order: user settings, managed
/// settings plus its sorted `managed-settings.d` drop-ins, then project and
/// project-local settings under `working_dir`.
fn claude_settings_paths(
    working_dir: Option<&str>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut settings_paths = Vec::new();
    if let Some(home) = env("USERPROFILE").or_else(|| env("HOME")) {
        settings_paths.push(PathBuf::from(home).join(".claude").join("settings.json"));
    }
    if let Some(program_files) = env("ProgramFiles") {
        let managed_root = PathBuf::from(program_files).join("ClaudeCode");
        settings_paths.push(managed_root.join("managed-settings.json"));
        if let Ok(entries) = fs::read_dir(managed_root.join("managed-settings.d")) {
            let mut drop_ins = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
                .collect::<Vec<_>>();
            drop_ins.sort();
            settings_paths.extend(drop_ins);
        }
    }
    if let Some(working_dir) = working_dir {
        settings_paths.push(
            PathBuf::from(working_dir)
                .join(".claude")
                .join("settings.json"),
        );
        settings_paths.push(
            PathBuf::from(working_dir)
                .join(".claude")
                .join("settings.local.json"),
        );
    }
    settings_paths
}

/// True when the merged Claude env routes to a non-Anthropic provider
/// (custom base URL, Bedrock, Vertex or Foundry).
fn claude_custom_provider(configured_env: &BTreeMap<String, String>) -> bool {
    configured_env
        .get("ANTHROPIC_BASE_URL")
        .is_some_and(|value| !value.trim().is_empty())
        || [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ]
        .iter()
        .any(|name| {
            configured_env.get(*name).is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
        })
}

fn merge_claude_model_settings(
    settings: &Value,
    configured_env: &mut BTreeMap<String, String>,
    configured_models: &mut BTreeSet<String>,
) {
    if let Some(env) = settings.get("env").and_then(Value::as_object) {
        for (name, value) in env {
            if let Some(value) = value.as_str() {
                configured_env.insert(name.clone(), value.to_owned());
            }
        }
    }
    for key in ["model", "fallbackModel"] {
        if let Some(model) = settings.get(key).and_then(Value::as_str) {
            configured_models.insert(model.to_owned());
        }
    }
    if let Some(available_models) = settings.get("availableModels").and_then(Value::as_array) {
        for model in available_models.iter().filter_map(Value::as_str) {
            configured_models.insert(model.to_owned());
        }
    }
    if let Some(overrides) = settings.get("modelOverrides").and_then(Value::as_object) {
        for (anthropic_id, provider_id) in overrides {
            if is_native_claude_model_shape(anthropic_id) {
                configured_models.insert(anthropic_id.clone());
            }
            if is_native_claude_model_shape(anthropic_id)
                && let Some(provider_id) = provider_id.as_str()
            {
                configured_models.insert(provider_id.to_owned());
            }
        }
    }
    if let Some(options) = settings
        .pointer("/modelPicker/options")
        .and_then(Value::as_array)
    {
        for option in options {
            let Some(model) = option.get("model").and_then(Value::as_str) else {
                continue;
            };
            let explicitly_mapped = option
                .get("behavesAs")
                .and_then(Value::as_str)
                .is_some_and(is_native_claude_model_shape);
            if explicitly_mapped {
                configured_models.insert(model.to_owned());
            }
        }
    }
}

fn validate_claude_model_with_context(
    model: &str,
    custom_provider: bool,
    configured_models: &[String],
) -> Result<(), ErrorData> {
    if custom_provider
        || configured_models
            .iter()
            .any(|configured| configured == model)
    {
        return Ok(());
    }
    if is_native_claude_model_shape(model) {
        return Ok(());
    }
    Err(mcp_error(
        error_codes::TOOL_PARAMS_INVALID,
        format!(
            "act_spawn_agent refused invalid native Claude model {model:?} before creating spawn artifacts; use a documented alias/full Claude model id, configure the model in Claude settings, or omit model"
        ),
    ))
}

fn is_native_claude_model_shape(model: &str) -> bool {
    if matches!(
        model,
        "default"
            | "opus"
            | "opus[1m]"
            | "opusplan"
            | "sonnet"
            | "sonnet[1m]"
            | "haiku"
            | "fable"
            | "mythos"
    ) {
        return true;
    }
    let Some(suffix) = model.strip_prefix("claude-") else {
        return false;
    };
    let family_first = ["opus-", "sonnet-", "haiku-", "fable-", "mythos-"]
        .iter()
        .find_map(|family| suffix.strip_prefix(family))
        .is_some_and(|version| {
            let version = version.strip_suffix("[1m]").unwrap_or(version);
            version
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
                && version
                    .chars()
                    .all(|character| character.is_ascii_digit() || matches!(character, '-' | '.'))
        });
    if family_first {
        return true;
    }
    let parts = suffix.split('-').collect::<Vec<_>>();
    parts.first().is_some_and(|part| {
        !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
    }) && parts
        .iter()
        .skip(1)
        .any(|part| matches!(*part, "opus" | "sonnet" | "haiku" | "fable" | "mythos"))
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.chars().all(|character| {
                    character.is_ascii_digit()
                        || character == '.'
                        || matches!(*part, "opus" | "sonnet" | "haiku" | "fable" | "mythos")
                })
        })
}

fn validate_spawn_target(target: &Option<ActSpawnAgentTarget>) -> Result<(), ErrorData> {
    match target {
        Some(
            ActSpawnAgentTarget::Window { window_hwnd }
            | ActSpawnAgentTarget::Cdp { window_hwnd, .. },
        ) => {
            validate_target_window(*window_hwnd)?;
        }
        None => {}
    }
    Ok(())
}

pub(super) fn resolve_agent_working_dir(working_dir: Option<&str>) -> Result<PathBuf, ErrorData> {
    let path = match working_dir {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("act_spawn_agent failed to read daemon current directory: {error}"),
            )
        })?,
    };
    let canonical = fs::canonicalize(&path).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_spawn_agent working_dir {path:?} could not be resolved: {error}"),
        )
    })?;
    if !canonical.is_dir() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_spawn_agent working_dir {canonical:?} is not a directory"),
        ));
    }
    Ok(canonical)
}

fn prepare_agent_spawn_files(
    spawn_id: &str,
    params: &ActSpawnAgentParams,
    working_dir: &Path,
) -> Result<AgentSpawnFiles, ErrorData> {
    prepare_agent_spawn_files_in(&agent_spawn_root_dir()?, spawn_id, params, working_dir)
}

/// Same as [`prepare_agent_spawn_files`] but with an explicit spawn root, so callers
/// (notably tests) never need to mutate the process-wide `LOCALAPPDATA`.
fn prepare_agent_spawn_files_in(
    root: &Path,
    spawn_id: &str,
    params: &ActSpawnAgentParams,
    working_dir: &Path,
) -> Result<AgentSpawnFiles, ErrorData> {
    let agent_kind = params.effective_cli()?;
    let log_dir = root.join(spawn_id);
    fs::create_dir_all(&log_dir).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            format!(
                "act_spawn_agent failed to create log directory {}: {error}",
                log_dir.display()
            ),
        )
    })?;
    let prompt_path = log_dir.join("prompt.txt");
    let stdout_path = log_dir.join("stdout.jsonl");
    let stderr_path = log_dir.join("stderr.log");
    let final_message_path = log_dir.join("final-message.txt");
    let completion_status_path = log_dir.join("completion-status.json");
    let task_started_path = log_dir.join("task-started.json");
    let task_started_script_path = log_dir.join("write-task-started.ps1");
    let debug_path =
        (agent_kind == ActSpawnAgentCli::Claude).then(|| log_dir.join("claude-debug.log"));
    let mcp_config_path =
        (agent_kind == ActSpawnAgentCli::Claude).then(|| log_dir.join("claude-mcp-config.json"));
    let hook_settings_path =
        (agent_kind == ActSpawnAgentCli::Claude).then(|| log_dir.join("claude-hook-settings.json"));
    let notify_script_path =
        (agent_kind == ActSpawnAgentCli::Codex).then(|| log_dir.join("codex-notify.ps1"));
    let codex_app_server_runner_path = (agent_kind == ActSpawnAgentCli::Codex)
        .then(|| log_dir.join("codex-app-server-runner.ps1"));
    let codex_app_server_control_path =
        (agent_kind == ActSpawnAgentCli::Codex).then(|| log_dir.join("codex-control.json"));
    let codex_app_server_events_path = (agent_kind == ActSpawnAgentCli::Codex)
        .then(|| log_dir.join("codex-app-server-events.jsonl"));
    let codex_app_server_stdout_path = (agent_kind == ActSpawnAgentCli::Codex)
        .then(|| log_dir.join("codex-app-server.stdout.log"));
    let codex_app_server_stderr_path = (agent_kind == ActSpawnAgentCli::Codex)
        .then(|| log_dir.join("codex-app-server.stderr.log"));
    let local_model_runner_path = agent_kind
        .is_local_model()
        .then(|| log_dir.join("local-model-runner.json"));

    let prompt = build_agent_spawn_prompt(
        spawn_id,
        params,
        working_dir,
        &task_started_path,
        &task_started_script_path,
    )?;
    fs::write(&prompt_path, prompt).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            format!(
                "act_spawn_agent failed to write prompt file {}: {error}",
                prompt_path.display()
            ),
        )
    })?;
    if let Some(config_path) = &mcp_config_path {
        let config = json!({
            "mcpServers": {
                "synapse": {
                    "type": "http",
                    "url": params.mcp_url,
                    "headers": {
                        "Authorization": "Bearer ${SYNAPSE_BEARER_TOKEN}",
                        // Lets the approval facade's delegated gate attribute a permission
                        // request to THIS spawn (#927); the bearer is shared
                        // across spawns and can't distinguish them. Read by
                        // server::permission_gate::SPAWN_ID_HEADER (case-insensitive).
                        "X-Synapse-Spawn-Id": spawn_id
                    },
                    // Per-server tool-call wall-clock (ms). The approval facade
                    // call blocks while a human decides; Claude's default
                    // MCP_TOOL_TIMEOUT (~28h) is plenty, but we pin 30 min here
                    // so a forgotten approval can't pin the agent forever — the
                    // gate itself denies after its default wait (#303), well
                    // inside this wall.
                    "timeout": 1_800_000
                }
            }
        });
        let encoded = serde_json::to_vec_pretty(&config).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("act_spawn_agent failed to encode Claude MCP config: {error}"),
            )
        })?;
        fs::write(config_path, encoded).map_err(|error| {
            mcp_error(
                error_codes::STORAGE_WRITE_FAILED,
                format!(
                    "act_spawn_agent failed to write Claude MCP config {}: {error}",
                    config_path.display()
                ),
            )
        })?;
    }

    if let Some(settings_path) = &hook_settings_path {
        let settings =
            build_claude_hook_settings(spawn_id, &params.mcp_url, params.require_approval_gate)?;
        let encoded = serde_json::to_vec_pretty(&settings).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("act_spawn_agent failed to encode Claude hook settings: {error}"),
            )
        })?;
        fs::write(settings_path, encoded).map_err(|error| {
            mcp_error(
                error_codes::STORAGE_WRITE_FAILED,
                format!(
                    "act_spawn_agent failed to write Claude hook settings {}: {error}",
                    settings_path.display()
                ),
            )
        })?;
    }
    if let Some(script_path) = &notify_script_path {
        let script = build_codex_notify_script(spawn_id, &params.mcp_url, &log_dir)?;
        fs::write(script_path, script).map_err(|error| {
            mcp_error(
                error_codes::STORAGE_WRITE_FAILED,
                format!(
                    "act_spawn_agent failed to write Codex notify script {}: {error}",
                    script_path.display()
                ),
            )
        })?;
    }
    if let Some(runner_path) = &codex_app_server_runner_path {
        fs::write(runner_path, CODEX_APP_SERVER_RUNNER_SCRIPT).map_err(|error| {
            mcp_error(
                error_codes::STORAGE_WRITE_FAILED,
                format!(
                    "act_spawn_agent failed to write Codex app-server runner {}: {error}",
                    runner_path.display()
                ),
            )
        })?;
    }

    // Spawn manifest: the authoritative record of which CLI and (when the
    // operator pinned one) which model this spawn was launched with. The
    // transcript ingester reads it to attribute cost — indispensable for Codex,
    // whose `exec --json` stream carries no model id (#949).
    let manifest_path = log_dir.join(AGENT_SPAWN_MANIFEST_FILENAME);
    let manifest = build_spawn_manifest(spawn_id, params, working_dir)?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("act_spawn_agent failed to encode spawn manifest: {error}"),
        )
    })?;
    fs::write(&manifest_path, manifest_bytes).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            format!(
                "act_spawn_agent failed to write spawn manifest {}: {error}",
                manifest_path.display()
            ),
        )
    })?;

    Ok(AgentSpawnFiles {
        log_dir,
        prompt_path,
        stdout_path,
        stderr_path,
        final_message_path,
        completion_status_path,
        task_started_path,
        task_started_script_path,
        debug_path,
        mcp_config_path,
        hook_settings_path,
        notify_script_path,
        codex_app_server_runner_path,
        codex_app_server_control_path,
        codex_app_server_events_path,
        codex_app_server_stdout_path,
        codex_app_server_stderr_path,
        local_model_runner_path,
    })
}

/// Derives the push-telemetry ingress endpoint from the MCP URL the spawned
/// agent is wired to. The daemon serves both from one origin, so anything
/// other than a `/mcp`-suffixed URL is a caller error, not a guessing game.
pub(crate) fn agent_event_ingress_url(
    spawn_id: &str,
    mcp_url: &str,
    source: &str,
) -> Result<String, ErrorData> {
    let Some(base) = mcp_url.strip_suffix("/mcp") else {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "act_spawn_agent cannot derive the /agent-events ingress URL: mcp_url {mcp_url:?} does not end with \"/mcp\""
            ),
        ));
    };
    if base.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_spawn_agent mcp_url {mcp_url:?} has no scheme/authority before \"/mcp\""),
        ));
    }
    Ok(format!(
        "{base}/agent-events?spawn_id={spawn_id}&source={source}"
    ))
}

/// Claude Code `--settings` payload subscribing the daemon's ingress to the
/// hook events listed in
/// [`super::agent_event_ingress::CLAUDE_HOOK_SUBSCRIBED_EVENTS`]. Uses the
/// CLI's native HTTP hooks (verified on Claude Code 2.1.176): no per-event
/// child process, bearer injected via `allowedEnvVars` interpolation, and
/// delivery failures are non-blocking for the agent.
fn build_claude_hook_settings(
    spawn_id: &str,
    mcp_url: &str,
    require_approval_gate: bool,
) -> Result<Value, ErrorData> {
    let ingress_url = agent_event_ingress_url(spawn_id, mcp_url, "claude_code_hooks")?;
    let hook_entry = json!({
        "type": "http",
        "url": ingress_url,
        "headers": { "Authorization": "Bearer $SYNAPSE_BEARER_TOKEN" },
        "allowedEnvVars": ["SYNAPSE_BEARER_TOKEN"],
        // Localhost POST; anything slower than this means the daemon is
        // gone and the agent should not crawl through its turn.
        "timeout": 5,
    });
    let mut hooks = Map::new();
    for event in super::agent_event_ingress::CLAUDE_HOOK_SUBSCRIBED_EVENTS {
        hooks.insert(
            (*event).to_owned(),
            json!([{ "hooks": [hook_entry.clone()] }]),
        );
    }
    let mut settings = json!({
        "hooks": hooks,
        "allowedHttpHookUrls": [format!("{}*", ingress_url)],
        "httpHookAllowedEnvVars": ["SYNAPSE_BEARER_TOKEN"],
    });
    if require_approval_gate {
        // Static allow rules are consulted BEFORE the permission-prompt-tool.
        // Only public facade tool names are valid here: spawned Claude runs on
        // the same public (`PUBLIC_TOOL_LIMIT`) normal-agent tool surface as production clients.
        // Anything unmatched falls through to mcp__synapse__approval, which
        // delegates to the hidden approval_gate implementation internally.
        let mut allow_rules: Vec<String> = CLAUDE_AUTO_ALLOW_RULES
            .iter()
            .map(|rule| (*rule).to_owned())
            .collect();
        allow_rules.extend(
            CLAUDE_COORDINATION_FACADE_MCP_TOOLS
                .iter()
                .map(|tool| format!("mcp__synapse__{tool}")),
        );
        settings["permissions"] = json!({ "allow": allow_rules });
    }
    Ok(settings)
}

/// Tools pre-approved in a gated Claude spawn's `--settings` so they skip the
/// approval facade round-trip. Mirrors the auto-allow side of
/// [`super::permission_policy`]; the delegated gate still backstops anything
/// unmatched.
const CLAUDE_AUTO_ALLOW_RULES: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "LS",
    "NotebookRead",
    "NotebookEdit",
    "TodoWrite",
    "ExitPlanMode",
    "BashOutput",
    "Task",
    "mcp__synapse__health",
    "mcp__synapse__session",
    "mcp__synapse__target",
    "mcp__synapse__agent",
    "mcp__synapse__approval",
    "Edit",
    "Write",
    "MultiEdit",
    "Bash(ls:*)",
    "Bash(cat:*)",
    "Bash(pwd)",
    "Bash(echo:*)",
    "Bash(head:*)",
    "Bash(tail:*)",
    "Bash(wc:*)",
    "Bash(grep:*)",
    "Bash(rg:*)",
    "Bash(find:*)",
    "Bash(which:*)",
    "Bash(git status:*)",
    "Bash(git diff:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git branch:*)",
    "Bash(cargo build:*)",
    "Bash(cargo check:*)",
    "Bash(cargo test:*)",
    "Bash(cargo clippy:*)",
    "Bash(cargo fmt:*)",
];

const CLAUDE_COORDINATION_FACADE_MCP_TOOLS: &[&str] = &["workspace"];

/// Codex `notify` program: receives the notification JSON as its final argv
/// argument and POSTs it verbatim to the ingress. Codex spawns it
/// fire-and-forget, so delivery failures are persisted to
/// `notify-errors.log` in the spawn directory — local evidence instead of a
/// silent drop.
fn build_codex_notify_script(
    spawn_id: &str,
    mcp_url: &str,
    log_dir: &Path,
) -> Result<String, ErrorData> {
    let ingress_url = agent_event_ingress_url(spawn_id, mcp_url, "codex_notify")?;
    let error_log = ps_single_quoted_path(&log_dir.join("notify-errors.log"));
    Ok(format!(
        "$ErrorActionPreference = 'Stop'\n\
try {{\n\
    $payload = $args[-1]\n\
    if ([string]::IsNullOrWhiteSpace($payload)) {{ throw 'codex notify payload argument is missing' }}\n\
    $token = $env:SYNAPSE_BEARER_TOKEN\n\
    if ([string]::IsNullOrWhiteSpace($token)) {{ throw 'SYNAPSE_BEARER_TOKEN is not set in the codex notify environment' }}\n\
    Invoke-RestMethod -Method Post -Uri {ingress_url} -Headers @{{ Authorization = \"Bearer $token\" }} -ContentType 'application/json' -Body $payload -TimeoutSec 5 | Out-Null\n\
}} catch {{\n\
    Add-Content -Path {error_log} -Value (\"{{0}}|codex-notify-post-failed|{{1}}\" -f (Get-Date -Format o), $_.Exception.Message)\n\
    exit 1\n\
}}\n",
        ingress_url = ps_single_quote(&ingress_url),
        error_log = error_log,
    ))
}

pub(crate) fn agent_spawn_root_dir() -> Result<PathBuf, ErrorData> {
    let local_appdata = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            "act_spawn_agent requires LOCALAPPDATA to create per-agent log directories",
        )
    })?;
    Ok(PathBuf::from(local_appdata)
        .join("Synapse")
        .join("agent-spawns"))
}

fn build_agent_spawn_prompt(
    spawn_id: &str,
    params: &ActSpawnAgentParams,
    working_dir: &Path,
    task_started_path: &Path,
    _task_started_script_path: &Path,
) -> Result<String, ErrorData> {
    let agent_kind = params.effective_cli()?;
    if agent_kind.is_local_model() {
        let assigned_prompt = params.prompt.as_deref().unwrap_or("").trim();
        if assigned_prompt.is_empty() {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "act_spawn_agent local_model prompt must not be empty",
            ));
        }
        return Ok(assigned_prompt.to_owned());
    }
    let target_instruction = match &params.target {
        Some(target) => {
            let target_json = serde_json::to_string(target).map_err(|error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("act_spawn_agent failed to encode target prompt JSON: {error}"),
                )
            })?;
            format!(
                "3. Call the real Synapse MCP target facade with exactly this JSON: {{\"operation\":\"set\",\"target\":{target_json}}}\n4. Call the real Synapse MCP target facade with exactly this JSON: {{\"operation\":\"get\"}} and verify its current target exactly matches that JSON."
            )
        }
        None => {
            "3. Call the real Synapse MCP target facade with exactly this JSON: {\"operation\":\"get\"} and report the returned session_id/current target.".to_owned()
        }
    };
    let assigned_prompt = params.prompt.as_deref().unwrap_or("").trim();
    if assigned_prompt.is_empty() {
        let mode = if params.template_id.is_some() {
            "template"
        } else {
            "direct spawn"
        };
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_spawn_agent {mode} prompt must not be empty"),
        ));
    }
    let assigned_block = format!(
        "After the provisioning checks pass, perform this assigned task:\n{assigned_prompt}"
    );
    let hold_instruction = if params.hold_open_ms == 0 {
        "Do not add an artificial hold-open sleep after completing the provisioning checks and assigned task.".to_owned()
    } else {
        format!(
            "After the provisioning checks and assigned task, keep this primary process alive for at least {} ms using Start-Sleep -Milliseconds {}, then finish.",
            params.hold_open_ms, params.hold_open_ms
        )
    };
    let task_started_path_display = task_started_path.display().to_string();
    Ok(format!(
        "You are a primary {cli} agent spawned by Synapse act_spawn_agent.\n\
Spawn ID: {spawn_id}\n\
Working directory: {working_dir}\n\
Windows shell contract:\n\
- Your local shell commands run under PowerShell on Windows, not Bash.\n\
- Do not use Bash heredocs such as `python - <<'PY'`. For multi-line Python, pipe a PowerShell here-string into Python:\n\
  @'\n\
  print(\"ok\")\n\
  '@ | python -\n\
- Use `Start-Sleep -Milliseconds N` for sleeps.\n\
- Write evidence/report files as UTF-8.\n\
Mandatory provisioning checks:\n\
1. Use the real Synapse MCP health tool through your normal configured MCP client. Do not use curl, direct HTTP, helper scripts, or local storage writes as a substitute.\n\
2. Use the real Synapse MCP session facade with exactly this JSON through your normal configured MCP client: {{\"operation\":\"list\"}}\n\
{target_instruction}\n\
5. If any Synapse MCP tool is missing or fails, stop and report the exact tool/error.\n\
6. Before performing the assigned task or hold-open sleep, write the required task-start readiness artifact to: {task_started_path}\n\
   Call the real Synapse MCP agent facade with exactly this JSON: {{\"operation\":\"task_started\",\"task_started\":{{\"spawn_id\":\"{spawn_id}\"}}}}\n\
   Verify the tool response has ok=true, session_id equal to this spawned MCP session id, and task_started_path equal to {task_started_path}.\n\
   If the facade call is missing or fails, stop and report the exact tool/error; do not use a helper script or direct file write.\n\
7. In your final response, include one compact JSON object containing spawn_id, health_ok, session_id, target_ok, task_started_path, and any error.\n\
\n\
{assigned_block}\n\
\n\
{hold_instruction}\n",
        cli = agent_kind.as_str(),
        spawn_id = spawn_id,
        working_dir = working_dir.display(),
        target_instruction = target_instruction,
        task_started_path = task_started_path_display,
        assigned_block = assigned_block,
        hold_instruction = hold_instruction,
    ))
}

fn write_agent_spawn_task_started_from_session(
    spawn_id: &str,
    session_id: &str,
) -> Result<AgentSpawnTaskStartedResponse, ErrorData> {
    validate_agent_spawn_id_path_segment(spawn_id)?;
    let root = agent_spawn_root_dir()?;
    let log_dir = root.join(spawn_id);
    if !log_dir.is_dir() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "agent_spawn_task_started refused unknown spawn_id {spawn_id:?}: no spawn directory at {}",
                log_dir.display()
            ),
        ));
    }

    let manifest = read_agent_spawn_manifest(&log_dir, spawn_id)?;
    let agent_kind = agent_kind_from_spawn_manifest(&manifest)?;
    let assigned_prompt_present = manifest
        .get("assigned_prompt_present")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let task_started_path = log_dir.join("task-started.json");

    if task_started_path.is_file() {
        let existing = read_task_started_value_strict(&task_started_path)?;
        ensure_existing_task_started_claim(
            &existing,
            spawn_id,
            agent_kind,
            session_id,
            assigned_prompt_present,
            &task_started_path,
        )?;
        return response_from_task_started_artifact(
            spawn_id,
            agent_kind,
            session_id,
            &task_started_path,
            existing,
        );
    }

    let artifact = build_agent_spawn_task_started_artifact(
        spawn_id,
        agent_kind,
        session_id,
        assigned_prompt_present,
        &task_started_path,
    );
    write_task_started_artifact_atomically(&task_started_path, &artifact)?;
    let readback = read_task_started_value_strict(&task_started_path)?;
    if readback != artifact {
        return Err(mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            format!(
                "agent_spawn_task_started wrote {} but readback did not match the intended artifact",
                task_started_path.display()
            ),
        ));
    }
    response_from_task_started_artifact(
        spawn_id,
        agent_kind,
        session_id,
        &task_started_path,
        readback,
    )
}

fn validate_agent_spawn_id_path_segment(spawn_id: &str) -> Result<(), ErrorData> {
    if !spawn_id.starts_with("agent-spawn-") {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("spawn_id must start with \"agent-spawn-\", got {spawn_id:?}"),
        ));
    }
    if spawn_id.len() > 128 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("spawn_id exceeds 128 chars ({})", spawn_id.len()),
        ));
    }
    if !spawn_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "spawn_id must contain only ASCII alphanumerics and dashes",
        ));
    }
    Ok(())
}

fn read_agent_spawn_manifest(log_dir: &Path, spawn_id: &str) -> Result<Value, ErrorData> {
    let manifest_path = log_dir.join(AGENT_SPAWN_MANIFEST_FILENAME);
    let bytes = fs::read(&manifest_path).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_READ_FAILED,
            format!(
                "agent_spawn_task_started failed to read spawn manifest {}: {error}",
                manifest_path.display()
            ),
        )
    })?;
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_READ_FAILED,
            format!(
                "agent_spawn_task_started failed to parse spawn manifest {}: {error}",
                manifest_path.display()
            ),
        )
    })?;
    if manifest.get("spawn_id").and_then(Value::as_str) != Some(spawn_id) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "agent_spawn_task_started refused spawn_id {spawn_id:?}: manifest spawn_id mismatch in {}",
                manifest_path.display()
            ),
        ));
    }
    Ok(manifest)
}

fn agent_kind_from_spawn_manifest(manifest: &Value) -> Result<ActSpawnAgentCli, ErrorData> {
    let cli = manifest
        .get("cli")
        .or_else(|| manifest.get("kind"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "agent_spawn_task_started refused spawn manifest without cli/kind",
            )
        })?;
    match cli {
        "codex" => Ok(ActSpawnAgentCli::Codex),
        "claude" => Ok(ActSpawnAgentCli::Claude),
        "local_model" => Ok(ActSpawnAgentCli::LocalModel),
        other => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("agent_spawn_task_started refused unsupported spawn cli {other:?}"),
        )),
    }
}

fn build_agent_spawn_task_started_artifact(
    spawn_id: &str,
    agent_kind: ActSpawnAgentCli,
    session_id: &str,
    assigned_prompt_present: bool,
    task_started_path: &Path,
) -> Value {
    json!({
        "schema_version": 1,
        "spawn_id": spawn_id,
        "cli": agent_kind.as_str(),
        "session_id": session_id,
        "status": "started",
        "health_ok": true,
        "target_ok": true,
        "assigned_prompt_present": assigned_prompt_present,
        "task_started_path": task_started_path.display().to_string(),
        "started_at_unix_ms": unix_time_ms_now(),
        "readiness_source": "agent_spawn_task_started_tool",
    })
}

fn read_task_started_value_strict(path: &Path) -> Result<Value, ErrorData> {
    let bytes = fs::read(path).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_READ_FAILED,
            format!(
                "agent_spawn_task_started failed to read {}: {error}",
                path.display()
            ),
        )
    })?;
    if bytes.is_empty() {
        return Err(mcp_error(
            error_codes::STORAGE_READ_FAILED,
            format!("agent_spawn_task_started found empty {}", path.display()),
        ));
    }
    let json_bytes = bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(bytes.as_slice());
    serde_json::from_slice(json_bytes).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_READ_FAILED,
            format!(
                "agent_spawn_task_started failed to parse {}: {error}",
                path.display()
            ),
        )
    })
}

fn ensure_existing_task_started_claim(
    existing: &Value,
    spawn_id: &str,
    agent_kind: ActSpawnAgentCli,
    session_id: &str,
    assigned_prompt_present: bool,
    task_started_path: &Path,
) -> Result<(), ErrorData> {
    let expected_path = task_started_path.display().to_string();
    let mut errors = Vec::new();
    if existing.get("schema_version").and_then(Value::as_u64) != Some(1) {
        errors.push("schema_version mismatch");
    }
    if existing.get("spawn_id").and_then(Value::as_str) != Some(spawn_id) {
        errors.push("spawn_id mismatch");
    }
    if existing.get("cli").and_then(Value::as_str) != Some(agent_kind.as_str()) {
        errors.push("cli mismatch");
    }
    if existing.get("session_id").and_then(Value::as_str) != Some(session_id) {
        errors.push("session_id mismatch");
    }
    if existing.get("status").and_then(Value::as_str) != Some("started") {
        errors.push("status mismatch");
    }
    if existing.get("health_ok").and_then(Value::as_bool) != Some(true) {
        errors.push("health_ok mismatch");
    }
    if existing.get("target_ok").and_then(Value::as_bool) != Some(true) {
        errors.push("target_ok mismatch");
    }
    if existing
        .get("assigned_prompt_present")
        .and_then(Value::as_bool)
        != Some(assigned_prompt_present)
    {
        errors.push("assigned_prompt_present mismatch");
    }
    if existing.get("task_started_path").and_then(Value::as_str) != Some(expected_path.as_str()) {
        errors.push("task_started_path mismatch");
    }
    if existing
        .get("started_at_unix_ms")
        .and_then(Value::as_u64)
        .is_none_or(|value| value == 0)
    {
        errors.push("started_at_unix_ms missing");
    }
    if !errors.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "agent_spawn_task_started refused to overwrite existing readiness artifact at {}: {}",
                task_started_path.display(),
                errors.join(", ")
            ),
        ));
    }
    Ok(())
}

fn write_task_started_artifact_atomically(path: &Path, artifact: &Value) -> Result<(), ErrorData> {
    let bytes = serde_json::to_vec_pretty(artifact).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("agent_spawn_task_started failed to encode artifact: {error}"),
        )
    })?;
    let temp_path = path.with_file_name(format!(
        "{}.tmp.{}.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("task-started.json"),
        std::process::id(),
        unix_time_ms_now()
    ));
    fs::write(&temp_path, bytes).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            format!(
                "agent_spawn_task_started failed to write temp artifact {}: {error}",
                temp_path.display()
            ),
        )
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        mcp_error(
            error_codes::STORAGE_WRITE_FAILED,
            format!(
                "agent_spawn_task_started failed to move temp artifact {} to {}: {error}",
                temp_path.display(),
                path.display()
            ),
        )
    })
}

fn response_from_task_started_artifact(
    spawn_id: &str,
    agent_kind: ActSpawnAgentCli,
    session_id: &str,
    task_started_path: &Path,
    artifact: Value,
) -> Result<AgentSpawnTaskStartedResponse, ErrorData> {
    let started_at_unix_ms = artifact
        .get("started_at_unix_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            mcp_error(
                error_codes::STORAGE_READ_FAILED,
                format!(
                    "agent_spawn_task_started read {} without started_at_unix_ms",
                    task_started_path.display()
                ),
            )
        })?;
    Ok(AgentSpawnTaskStartedResponse {
        ok: true,
        spawn_id: spawn_id.to_owned(),
        session_id: session_id.to_owned(),
        cli: agent_kind,
        task_started_path: task_started_path.display().to_string(),
        started_at_unix_ms,
        readiness_source: "agent_spawn_task_started_tool".to_owned(),
        artifact,
    })
}

fn agent_spawn_powershell_script(
    params: &ActSpawnAgentParams,
    files: &AgentSpawnFiles,
    working_dir: &Path,
) -> Result<String, ErrorData> {
    let agent_kind = params.effective_cli()?;
    let prompt_path = ps_single_quoted_path(&files.prompt_path);
    let stdout_path = ps_single_quoted_path(&files.stdout_path);
    let stderr_path = ps_single_quoted_path(&files.stderr_path);
    let final_message_path = ps_single_quoted_path(&files.final_message_path);
    let completion_status_path = ps_single_quoted_path(&files.completion_status_path);
    let task_started_path = ps_single_quoted_path(&files.task_started_path);
    let working_dir = ps_single_quoted_path(working_dir);
    let command_body = match agent_kind {
        ActSpawnAgentCli::Codex => {
            let Some(notify_script_path) = files.notify_script_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Codex notify script path",
                ));
            };
            let Some(runner_path) = files.codex_app_server_runner_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Codex app-server runner path",
                ));
            };
            let Some(control_path) = files.codex_app_server_control_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Codex app-server control path",
                ));
            };
            let Some(events_path) = files.codex_app_server_events_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Codex app-server events path",
                ));
            };
            let Some(app_stdout_path) = files.codex_app_server_stdout_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Codex app-server stdout path",
                ));
            };
            let Some(app_stderr_path) = files.codex_app_server_stderr_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Codex app-server stderr path",
                ));
            };
            let model_arg = params
                .model
                .as_deref()
                .map(|model| format!("$codexRunnerArgs['Model'] = {}\n", ps_single_quote(model)))
                .unwrap_or_default();
            let approval_gate_arg = if params.require_approval_gate {
                "$codexRunnerArgs['RequireApprovalGate'] = $true\n"
            } else {
                ""
            };
            format!(
                "$codexRunnerArgs = @{{\n\
    SpawnId = $spawnId\n\
    PromptPath = $spawnPromptPath\n\
    StdoutPath = $spawnStdoutPath\n\
    StderrPath = $spawnStderrPath\n\
    FinalMessagePath = $spawnFinalMessagePath\n\
    ControlPath = {control_path}\n\
    EventsPath = {events_path}\n\
    AppServerStdoutPath = {app_stdout_path}\n\
    AppServerStderrPath = {app_stderr_path}\n\
    WorkingDir = {working_dir}\n\
    McpUrl = {mcp_url}\n\
    NotifyScriptPath = {notify_script_path}\n\
}}\n\
{model_arg}\
{approval_gate_arg}\
& {runner_path} @codexRunnerArgs\n\
",
                runner_path = ps_single_quoted_path(runner_path),
                control_path = ps_single_quoted_path(control_path),
                events_path = ps_single_quoted_path(events_path),
                app_stdout_path = ps_single_quoted_path(app_stdout_path),
                app_stderr_path = ps_single_quoted_path(app_stderr_path),
                working_dir = working_dir,
                mcp_url = ps_single_quote(&params.mcp_url),
                notify_script_path = ps_single_quoted_path(notify_script_path),
                model_arg = model_arg,
                approval_gate_arg = approval_gate_arg,
            )
        }
        ActSpawnAgentCli::Claude => {
            let Some(debug_path) = files.debug_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Claude debug path",
                ));
            };
            let Some(mcp_config_path) = files.mcp_config_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Claude MCP config path",
                ));
            };
            let Some(hook_settings_path) = files.hook_settings_path.as_ref() else {
                return Err(mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: missing Claude hook settings path",
                ));
            };
            let debug_path = ps_single_quoted_path(debug_path);
            let mcp_config_path = ps_single_quoted_path(mcp_config_path);
            let hook_settings_path = ps_single_quoted_path(hook_settings_path);
            let model_arg = params
                .model
                .as_deref()
                .map(|model| format!(",'--model',{}", ps_single_quote(model)))
                .unwrap_or_default();
            // #927: by default route Claude risky tool calls through the human
            // Approvals inbox. `--permission-mode default` consults the static
            // `permissions.allow` rules in the --settings file first (so safe
            // tools never pause), then delegates anything unmatched to the
            // public approval facade, which delegates to the gate and blocks
            // until a human decides.
            // The auto-approve-everything behavior is opt-in via
            // require_approval_gate=false (trusted unattended automation).
            let permission_args = if params.require_approval_gate {
                "'--permission-mode','default','--permission-prompt-tool','mcp__synapse__approval'"
            } else {
                "'--permission-mode','bypassPermissions'"
            };
            format!(
                "$claudeArgs = @('-p'{model_arg},'--verbose','--output-format','stream-json','--input-format','text',{permission_args},'--mcp-config',{mcp_config_path},'--strict-mcp-config','--settings',{hook_settings_path},'--add-dir',{working_dir},'--debug-file',{debug_path})\n\
$prompt | & claude @claudeArgs 1> {stdout_path} 2> {stderr_path}\n\
",
            )
        }
        ActSpawnAgentCli::LocalModel => {
            let exe = std::env::current_exe().map_err(|error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!(
                        "act_spawn_agent failed to resolve current synapse-mcp executable: {error}"
                    ),
                )
            })?;
            let model_ref = params.local_model_ref().ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "act_spawn_agent internal error: local_model command requested without model_ref",
                )
            })?;
            let timeout_ms = params
                .wait_timeout_ms
                .saturating_add(params.hold_open_ms)
                .max(120_000)
                .min(MAX_AGENT_SPAWN_WAIT_TIMEOUT_MS);
            let mut local_args = format!(
                "@('--mode','local-agent','--local-agent-model',{model_ref},'--local-agent-task-file',{prompt_path},'--local-agent-mcp-url',{mcp_url},'--local-agent-spawn-id',$spawnId,'--local-agent-log-dir',{log_dir},'--local-agent-timeout-ms','{timeout_ms}','--local-agent-hold-open-ms','{hold_open_ms}')",
                model_ref = ps_single_quote(model_ref),
                prompt_path = prompt_path,
                mcp_url = ps_single_quote(&params.mcp_url),
                log_dir = ps_single_quoted_path(&files.log_dir),
                timeout_ms = timeout_ms,
                hold_open_ms = params.hold_open_ms,
            );
            if let Some(target) = &params.target {
                let target_json = serde_json::to_string(target).map_err(|error| {
                    mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!(
                            "act_spawn_agent failed to encode local-model target JSON: {error}"
                        ),
                    )
                })?;
                local_args = format!(
                    "$localArgs = {local_args}\n$localArgs += @('--local-agent-target-json',{target_json})",
                    local_args = local_args,
                    target_json = ps_single_quote(&target_json),
                );
            } else {
                local_args = format!("$localArgs = {local_args}");
            }
            local_args
                .push_str("\n$localArgs += @('--local-agent-trusted-unattended-exact-contract')");
            format!(
                "{local_args}\n& {exe} @localArgs 1> {stdout_path} 2> {stderr_path}\n",
                local_args = local_args,
                exe = ps_single_quoted_path(&exe),
                stdout_path = stdout_path,
                stderr_path = stderr_path,
            )
        }
    };
    Ok(agent_spawn_wrapper_powershell(
        params,
        &prompt_path,
        &stdout_path,
        &stderr_path,
        &final_message_path,
        &completion_status_path,
        &task_started_path,
        &working_dir,
        &command_body,
    ))
}

fn agent_spawn_wrapper_powershell(
    params: &ActSpawnAgentParams,
    prompt_path: &str,
    stdout_path: &str,
    stderr_path: &str,
    final_message_path: &str,
    completion_status_path: &str,
    task_started_path: &str,
    working_dir: &str,
    command_body: &str,
) -> String {
    let agent_kind = params.effective_cli().ok();
    let cli = ps_single_quote(
        agent_kind
            .map(ActSpawnAgentCli::as_str)
            .unwrap_or("invalid"),
    );
    format!(
        "$ErrorActionPreference = 'Stop'\n\
$Utf8NoBom = [System.Text.UTF8Encoding]::new($false)\n\
$OutputEncoding = $Utf8NoBom\n\
$PSDefaultParameterValues['*:Encoding'] = 'utf8'\n\
try {{ [Console]::OutputEncoding = $Utf8NoBom }} catch {{}}\n\
try {{ [Console]::InputEncoding = $Utf8NoBom }} catch {{}}\n\
$env:PYTHONUTF8 = '1'\n\
$env:PYTHONIOENCODING = 'utf-8'\n\
$env:PYTHONUNBUFFERED = '1'\n\
Remove-Item Env:PYTHONLEGACYWINDOWSSTDIO -ErrorAction SilentlyContinue\n\
$spawnId = {spawn_id_expr}\n\
$spawnCli = {cli}\n\
$spawnWrapperProcessId = $PID\n\
$requestedHoldOpenMs = [int64]{hold_open_ms}\n\
$spawnPromptPath = {prompt_path}\n\
$spawnStdoutPath = {stdout_path}\n\
$spawnStderrPath = {stderr_path}\n\
$spawnFinalMessagePath = {final_message_path}\n\
$spawnCompletionStatusPath = {completion_status_path}\n\
$spawnTaskStartedPath = {task_started_path}\n\
$spawnStartedAtUnixMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()\n\
$spawnExitCode = 1\n\
$spawnTerminalStatus = 'wrapper_error'\n\
$spawnErrorMessage = $null\n\
$spawnFinalMessageSource = $null\n\
$spawnRecoveredFinalMessageWritten = $false\n\
$spawnRecoveredFinalMessageSource = $null\n\
$spawnResultIsError = $false\n\
\n\
function Get-SpawnFileLength([string]$Path) {{\n\
    if (Test-Path -LiteralPath $Path) {{ return [int64](Get-Item -LiteralPath $Path).Length }}\n\
    return [int64]0\n\
}}\n\
\n\
function Get-SpawnStdoutSummary([string]$Path) {{\n\
    $lineCount = 0\n\
    $lastEventType = $null\n\
    if (Test-Path -LiteralPath $Path) {{\n\
        foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {{\n\
            if ([string]::IsNullOrWhiteSpace($line)) {{ continue }}\n\
            $lineCount++\n\
            try {{\n\
                $event = $line | ConvertFrom-Json -ErrorAction Stop\n\
                if ($null -ne $event.type) {{ $lastEventType = [string]$event.type }}\n\
                elseif ($null -ne $event.item -and $null -ne $event.item.type) {{ $lastEventType = [string]$event.item.type }}\n\
            }} catch {{}}\n\
        }}\n\
    }}\n\
    return [pscustomobject]@{{ line_count = $lineCount; last_event_type = $lastEventType }}\n\
}}\n\
\n\
function Get-SpawnFinalAssistantTextFromStdout([string]$Path) {{\n\
    $finalText = $null\n\
    if (Test-Path -LiteralPath $Path) {{\n\
        foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {{\n\
            if ([string]::IsNullOrWhiteSpace($line)) {{ continue }}\n\
            try {{\n\
                $event = $line | ConvertFrom-Json -ErrorAction Stop\n\
                if ($event.type -eq 'result') {{ $script:spawnResultIsError = ($event.is_error -eq $true) }}\n\
                if ($null -ne $event.item -and $event.item.type -eq 'agent_message' -and $null -ne $event.item.text) {{\n\
                    $finalText = [string]$event.item.text\n\
                    $script:spawnRecoveredFinalMessageSource = 'stdout_jsonl_agent_message'\n\
                }} elseif ($event.type -eq 'agent_message' -and $null -ne $event.text) {{\n\
                    $finalText = [string]$event.text\n\
                    $script:spawnRecoveredFinalMessageSource = 'stdout_jsonl_agent_message'\n\
                }} elseif ($event.type -eq 'message' -and $event.role -eq 'assistant' -and $null -ne $event.content) {{\n\
                    if ($event.content -is [string]) {{\n\
                        $finalText = [string]$event.content\n\
                        $script:spawnRecoveredFinalMessageSource = 'stdout_jsonl_message'\n\
                    }} else {{\n\
                        $parts = @()\n\
                        foreach ($part in $event.content) {{\n\
                            if ($null -ne $part.text) {{ $parts += [string]$part.text }}\n\
                        }}\n\
                        if ($parts.Count -gt 0) {{\n\
                            $finalText = [string]::Join(\"`n\", $parts)\n\
                            $script:spawnRecoveredFinalMessageSource = 'stdout_jsonl_message'\n\
                        }}\n\
                    }}\n\
                }} elseif ($event.type -eq 'assistant' -and $null -ne $event.message -and $event.message.role -eq 'assistant' -and $null -ne $event.message.content) {{\n\
                    $parts = @()\n\
                    foreach ($part in $event.message.content) {{\n\
                        if ($null -ne $part.text) {{ $parts += [string]$part.text }}\n\
                    }}\n\
                    if ($parts.Count -gt 0) {{\n\
                        $finalText = [string]::Join(\"`n\", $parts)\n\
                        $script:spawnRecoveredFinalMessageSource = 'stdout_jsonl_claude_assistant_message'\n\
                    }}\n\
                }} elseif ($event.type -eq 'result' -and $null -ne $event.result) {{\n\
                    $finalText = [string]$event.result\n\
                    $script:spawnRecoveredFinalMessageSource = 'stdout_jsonl_result'\n\
                }}\n\
            }} catch {{}}\n\
        }}\n\
    }}\n\
    return $finalText\n\
}}\n\
\n\
function Write-SpawnRecoveredFinalMessage([string]$Text) {{\n\
    Set-Content -LiteralPath $spawnFinalMessagePath -Value $Text -Encoding UTF8\n\
    if ([string]::IsNullOrWhiteSpace($script:spawnRecoveredFinalMessageSource)) {{\n\
        $script:spawnFinalMessageSource = 'stdout_jsonl_recovered_final_text'\n\
    }} else {{\n\
        $script:spawnFinalMessageSource = $script:spawnRecoveredFinalMessageSource\n\
    }}\n\
    $script:spawnRecoveredFinalMessageWritten = $true\n\
}}\n\
\n\
function Write-SpawnFallbackFinalMessage([string]$Status, [int]$ExitCode, [string]$ErrorMessage) {{\n\
    $stdoutSummary = Get-SpawnStdoutSummary -Path $spawnStdoutPath\n\
    $fallback = [ordered]@{{\n\
        schema_version = 1\n\
        spawn_id = $spawnId\n\
        cli = $spawnCli\n\
        status = $Status\n\
        exit_code = $ExitCode\n\
        error_message = $ErrorMessage\n\
        message = 'No final assistant response artifact was produced by the spawned agent CLI; this file was written by the Synapse act_spawn_agent wrapper.'\n\
        stdout_path = $spawnStdoutPath\n\
        stderr_path = $spawnStderrPath\n\
        completion_status_path = $spawnCompletionStatusPath\n\
        task_started_path = $spawnTaskStartedPath\n\
        stdout_line_count = $stdoutSummary.line_count\n\
        last_stdout_event_type = $stdoutSummary.last_event_type\n\
    }}\n\
    $fallback | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $spawnFinalMessagePath -Encoding UTF8\n\
    $script:spawnFinalMessageSource = 'wrapper_fallback_json'\n\
}}\n\
\n\
function Write-SpawnCompletionStatus([string]$Status, [int]$ExitCode, [string]$ErrorMessage, [bool]$FallbackFinalMessageWritten) {{\n\
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()\n\
    $elapsed = [int64]($now - $spawnStartedAtUnixMs)\n\
    $stdoutSummary = Get-SpawnStdoutSummary -Path $spawnStdoutPath\n\
    $finalBytes = Get-SpawnFileLength -Path $spawnFinalMessagePath\n\
    $statusObject = [ordered]@{{\n\
        schema_version = 1\n\
        spawn_id = $spawnId\n\
        cli = $spawnCli\n\
        status = $Status\n\
        exit_code = $ExitCode\n\
        error_message = $ErrorMessage\n\
        wrapper_process_id = $spawnWrapperProcessId\n\
        wrapper_started_at_unix_ms = $spawnStartedAtUnixMs\n\
        completed_at_unix_ms = $now\n\
        elapsed_ms = $elapsed\n\
        requested_hold_open_ms = $requestedHoldOpenMs\n\
        hold_open_elapsed_ms_met = ($elapsed -ge $requestedHoldOpenMs)\n\
        final_message_path = $spawnFinalMessagePath\n\
        final_message_bytes = $finalBytes\n\
        final_message_present = ($finalBytes -gt 0)\n\
        final_message_source = $spawnFinalMessageSource\n\
        recovered_final_message_written = $spawnRecoveredFinalMessageWritten\n\
        fallback_final_message_written = $FallbackFinalMessageWritten\n\
        task_started_path = $spawnTaskStartedPath\n\
        task_started_bytes = (Get-SpawnFileLength -Path $spawnTaskStartedPath)\n\
        task_started_present = ((Get-SpawnFileLength -Path $spawnTaskStartedPath) -gt 0)\n\
        stdout_path = $spawnStdoutPath\n\
        stdout_line_count = $stdoutSummary.line_count\n\
        last_stdout_event_type = $stdoutSummary.last_event_type\n\
        stderr_path = $spawnStderrPath\n\
        stderr_bytes = (Get-SpawnFileLength -Path $spawnStderrPath)\n\
    }}\n\
    $statusObject | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $spawnCompletionStatusPath -Encoding UTF8\n\
}}\n\
\n\
function Invoke-SpawnHoldOpen {{\n\
    if ($requestedHoldOpenMs -le 0) {{ return }}\n\
    while ($true) {{\n\
        $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()\n\
        $elapsed = [int64]($now - $spawnStartedAtUnixMs)\n\
        $remaining = [int64]($requestedHoldOpenMs - $elapsed)\n\
        if ($remaining -le 0) {{ return }}\n\
        $sleepMs = [int][Math]::Min($remaining, 60000)\n\
        Start-Sleep -Milliseconds $sleepMs\n\
    }}\n\
}}\n\
\n\
try {{\n\
    Write-SpawnCompletionStatus -Status 'running' -ExitCode -1 -ErrorMessage $null -FallbackFinalMessageWritten:$false\n\
    Set-Location -LiteralPath {working_dir}\n\
    $prompt = Get-Content -Raw -LiteralPath $spawnPromptPath -Encoding UTF8\n\
{command_body}\
    $spawnExitCode = if ($null -eq $LASTEXITCODE) {{ 0 }} else {{ [int]$LASTEXITCODE }}\n\
    $spawnTerminalStatus = if ($spawnExitCode -eq 0) {{ 'completed' }} else {{ 'failed' }}\n\
}} catch {{\n\
    $spawnErrorMessage = $_.Exception.Message\n\
    $spawnExitCode = 1\n\
    $spawnTerminalStatus = 'wrapper_error'\n\
    try {{ Add-Content -LiteralPath $spawnStderrPath -Value (\"SYNAPSE_AGENT_SPAWN_WRAPPER_ERROR: \" + $spawnErrorMessage) -Encoding UTF8 }} catch {{}}\n\
}} finally {{\n\
    $finalBytesBeforeFallback = Get-SpawnFileLength -Path $spawnFinalMessagePath\n\
    $fallbackWritten = $false\n\
    if ($spawnTerminalStatus -eq 'completed' -and $finalBytesBeforeFallback -gt 0) {{\n\
        $finalStatus = 'ok'\n\
        $spawnFinalMessageSource = 'cli_output_file'\n\
    }} elseif ($spawnTerminalStatus -eq 'completed') {{\n\
        $recoveredFinalText = Get-SpawnFinalAssistantTextFromStdout -Path $spawnStdoutPath\n\
        if (-not [string]::IsNullOrWhiteSpace($recoveredFinalText)) {{\n\
            Write-SpawnRecoveredFinalMessage -Text $recoveredFinalText\n\
            if ($spawnResultIsError) {{\n\
                $finalStatus = 'failed'\n\
                $spawnErrorMessage = 'spawned agent CLI exited 0 but its final stream-json result event reported is_error=true'\n\
            }} else {{\n\
                $finalStatus = 'ok'\n\
                $spawnErrorMessage = $null\n\
            }}\n\
        }} else {{\n\
            $finalStatus = 'missing_final_response'\n\
            $spawnErrorMessage = 'spawned agent CLI exited 0 but did not write final-message.txt and no assistant message was recoverable from stdout.jsonl'\n\
            Write-SpawnFallbackFinalMessage -Status $finalStatus -ExitCode $spawnExitCode -ErrorMessage $spawnErrorMessage\n\
            $fallbackWritten = $true\n\
        }}\n\
    }} elseif ($spawnTerminalStatus -eq 'wrapper_error') {{\n\
        $finalStatus = 'wrapper_error'\n\
        Write-SpawnFallbackFinalMessage -Status $finalStatus -ExitCode $spawnExitCode -ErrorMessage $spawnErrorMessage\n\
        $fallbackWritten = $true\n\
    }} else {{\n\
        $finalStatus = 'failed'\n\
        if ($null -eq $spawnErrorMessage) {{ $spawnErrorMessage = \"spawned agent CLI exited with code $spawnExitCode\" }}\n\
        Write-SpawnFallbackFinalMessage -Status $finalStatus -ExitCode $spawnExitCode -ErrorMessage $spawnErrorMessage\n\
        $fallbackWritten = $true\n\
    }}\n\
    if ($finalStatus -eq 'ok') {{ Invoke-SpawnHoldOpen }}\n\
    Write-SpawnCompletionStatus -Status $finalStatus -ExitCode $spawnExitCode -ErrorMessage $spawnErrorMessage -FallbackFinalMessageWritten:$fallbackWritten\n\
}}\n\
exit $spawnExitCode\n",
        spawn_id_expr = "$env:SYNAPSE_AGENT_SPAWN_ID",
        cli = cli,
        hold_open_ms = params.hold_open_ms,
        prompt_path = prompt_path,
        stdout_path = stdout_path,
        stderr_path = stderr_path,
        final_message_path = final_message_path,
        completion_status_path = completion_status_path,
        task_started_path = task_started_path,
        working_dir = working_dir,
        command_body = command_body,
    )
}

fn ps_single_quoted_path(path: &Path) -> String {
    ps_single_quote(&path.display().to_string())
}

fn ps_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn agent_spawn_wait_deadline_from(
    start: Instant,
    wait_timeout_ms: u64,
) -> Result<Instant, ErrorData> {
    if wait_timeout_ms > MAX_AGENT_SPAWN_WAIT_TIMEOUT_MS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("act_spawn_agent wait_timeout_ms must be <= {MAX_AGENT_SPAWN_WAIT_TIMEOUT_MS}"),
        ));
    }
    start
        .checked_add(Duration::from_millis(wait_timeout_ms))
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "act_spawn_agent wait_timeout_ms {wait_timeout_ms} is too large for this host clock"
                ),
            )
        })
}

fn agent_spawn_deadline_remaining(deadline: Instant) -> Duration {
    deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_default()
}

async fn sleep_agent_spawn_poll(deadline: Instant) {
    let remaining = agent_spawn_deadline_remaining(deadline);
    if remaining.is_zero() {
        return;
    }
    let poll = Duration::from_millis(AGENT_SPAWN_POLL_INTERVAL_MS);
    tokio::time::sleep(if remaining < poll { remaining } else { poll }).await;
}

fn read_agent_spawn_task_start_artifact(
    files: &AgentSpawnFiles,
    params: &ActSpawnAgentParams,
    agent_kind: ActSpawnAgentCli,
    spawn_id: &str,
    matched: &MatchedSpawnSession,
) -> Result<Option<AgentSpawnTaskStartRead>, serde_json::Value> {
    let bytes = match fs::read(&files.task_started_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(json!({
                "reason": "task_start_artifact_read_failed",
                "task_started_path": files.task_started_path.display().to_string(),
                "error": error.to_string(),
            }));
        }
    };
    if bytes.is_empty() {
        return Err(json!({
            "reason": "task_start_artifact_empty",
            "task_started_path": files.task_started_path.display().to_string(),
        }));
    }
    let json_bytes = bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(bytes.as_slice());
    let value = serde_json::from_slice::<Value>(json_bytes).map_err(|error| {
        json!({
            "reason": "task_start_artifact_json_invalid",
            "task_started_path": files.task_started_path.display().to_string(),
            "bytes": bytes.len(),
            "error": error.to_string(),
            "text": String::from_utf8_lossy(&bytes).into_owned(),
        })
    })?;
    let Some(object) = value.as_object() else {
        return Err(json!({
            "reason": "task_start_artifact_not_object",
            "task_started_path": files.task_started_path.display().to_string(),
            "artifact": value,
        }));
    };

    let assigned_prompt_present = params
        .prompt
        .as_deref()
        .is_some_and(|prompt| !prompt.trim().is_empty());
    let expected_path = files.task_started_path.display().to_string();
    let mut validation_errors = Vec::new();

    if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
        validation_errors.push("schema_version must be 1");
    }
    if object.get("spawn_id").and_then(Value::as_str) != Some(spawn_id) {
        validation_errors.push("spawn_id mismatch");
    }
    if object.get("cli").and_then(Value::as_str) != Some(agent_kind.as_str()) {
        validation_errors.push("cli mismatch");
    }
    if object.get("session_id").and_then(Value::as_str) != Some(matched.session_id.as_str()) {
        validation_errors.push("session_id mismatch");
    }
    if object.get("status").and_then(Value::as_str) != Some("started") {
        validation_errors.push("status must be started");
    }
    if object.get("health_ok").and_then(Value::as_bool) != Some(true) {
        validation_errors.push("health_ok must be true");
    }
    if object.get("target_ok").and_then(Value::as_bool) != Some(true) {
        validation_errors.push("target_ok must be true");
    }
    if object
        .get("assigned_prompt_present")
        .and_then(Value::as_bool)
        != Some(assigned_prompt_present)
    {
        validation_errors.push("assigned_prompt_present mismatch");
    }
    if object.get("task_started_path").and_then(Value::as_str) != Some(expected_path.as_str()) {
        validation_errors.push("task_started_path mismatch");
    }
    let started_at_unix_ms = match object.get("started_at_unix_ms").and_then(Value::as_u64) {
        Some(value) if value > 0 => value,
        _ => {
            validation_errors.push("started_at_unix_ms must be a positive integer");
            0
        }
    };
    let readiness_source = match object.get("readiness_source").and_then(Value::as_str) {
        None => "task_start_artifact",
        Some("task_start_artifact") => "task_start_artifact",
        Some("agent_spawn_task_started_tool") => "agent_spawn_task_started_tool",
        Some(_) => {
            validation_errors.push(
                "readiness_source must be task_start_artifact or agent_spawn_task_started_tool",
            );
            "task_start_artifact"
        }
    };
    if matches!(
        agent_kind,
        ActSpawnAgentCli::Claude | ActSpawnAgentCli::Codex
    ) && readiness_source != "agent_spawn_task_started_tool"
    {
        validation_errors
            .push("readiness_source must be agent_spawn_task_started_tool for claude/codex spawns");
    }

    if !validation_errors.is_empty() {
        return Err(json!({
            "reason": "task_start_artifact_invalid",
            "task_started_path": expected_path,
            "validation_errors": validation_errors,
            "artifact": value,
        }));
    }

    Ok(Some(AgentSpawnTaskStartRead {
        started_at_unix_ms,
        readiness_source,
    }))
}

/// Daemon-OBSERVABLE evidence that a spawned agent began executing. This is
/// diagnostic/session-attribution evidence only; #1225 forbids treating it as
/// final spawn readiness without a live session plus task-start artifact.
fn agent_spawn_observed_task_progress(
    files: &AgentSpawnFiles,
    agent_kind: ActSpawnAgentCli,
) -> Option<&'static str> {
    // The agent emitted a final message: it executed the task to completion.
    if file_len(&files.final_message_path) > 0 {
        return Some("final_message_present");
    }
    // Codex: the app-server control artifact (written by the daemon-controlled
    // runner, not the model) shows a thread established and a turn underway.
    if agent_kind == ActSpawnAgentCli::Codex {
        if let Some(path) = files.codex_app_server_control_path.as_ref() {
            if let Some(value) = read_json_file_lossy(path) {
                let thread_present = value
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty());
                let turn_status = value
                    .get("turn_status")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // `starting`/`app_server_started` predate the agent's thread; any
                // later status means codex created its thread and is executing.
                let turn_underway = !matches!(turn_status, "" | "starting" | "app_server_started");
                if thread_present && turn_underway {
                    return Some("codex_control_thread_established");
                }
            }
        }
    }
    // The agent's stdout shows turn/assistant activity (local-model or codex):
    // it began reasoning or acting, which proves the task is underway.
    if let Some(tail) = tail_file_lossy(&files.stdout_path, AGENT_SPAWN_LOG_TAIL_BYTES) {
        if tail.contains("local.turn.started")
            || tail.contains("local.assistant.message")
            || tail.contains("\"turn.started\"")
            || tail.contains("\"item.completed\"")
        {
            return Some("stdout_turn_activity");
        }
    }
    None
}

/// Failure evidence is separate from readiness: a runner can fail before it
/// obtains a thread/turn ID. Missing or partially written artifacts are not
/// terminal evidence and must never shorten the normal readiness deadline.
fn read_agent_spawn_terminal_failure(
    files: &AgentSpawnFiles,
    agent_kind: ActSpawnAgentCli,
    spawn_id: &str,
    launched_at_unix_ms: u64,
    launcher_pid: u32,
) -> Option<Value> {
    if files.log_dir.file_name()?.to_str()? != spawn_id {
        return None;
    }
    let now = unix_time_ms_now();
    let read_artifact = |path: &Path| -> Option<Value> {
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes)).ok()
    };
    let fresh = |value: &Value, field: &str| {
        value
            .get(field)
            .and_then(Value::as_u64)
            .is_some_and(|at| at >= launched_at_unix_ms && at <= now)
    };
    if let Some(completion) = read_artifact(&files.completion_status_path)
        && completion.get("schema_version").and_then(Value::as_u64) == Some(1)
        && completion.get("spawn_id").and_then(Value::as_str) == Some(spawn_id)
        && completion.get("cli").and_then(Value::as_str) == Some(agent_kind.as_str())
        && completion.get("wrapper_process_id").and_then(Value::as_u64)
            == Some(u64::from(launcher_pid))
        && completion.get("task_started_path").and_then(Value::as_str)
            == Some(files.task_started_path.to_string_lossy().as_ref())
        && fresh(&completion, "wrapper_started_at_unix_ms")
        && fresh(&completion, "completed_at_unix_ms")
        && matches!(
            completion.get("status").and_then(Value::as_str),
            Some("failed" | "wrapper_error" | "missing_final_response")
        )
    {
        return Some(json!({
            "reason": "spawn_terminal_failure",
            "source": "completion_status",
            "artifact_path": files.completion_status_path,
            "failure": completion,
        }));
    }
    if agent_kind != ActSpawnAgentCli::Codex {
        return None;
    }
    let path = files.codex_app_server_control_path.as_ref()?;
    let control = read_artifact(path)?;
    if control.get("schema_version").and_then(Value::as_u64) != Some(1)
        || control.get("protocol").and_then(Value::as_str) != Some("codex_app_server_ws")
        || control.get("control_path").and_then(Value::as_str)
            != Some(path.to_string_lossy().as_ref())
        || control.get("events_path").and_then(Value::as_str)
            != Some(
                files
                    .codex_app_server_events_path
                    .as_ref()?
                    .to_string_lossy()
                    .as_ref(),
            )
        || !fresh(&control, "updated_at_unix_ms")
        || !matches!(
            control.get("turn_status").and_then(Value::as_str),
            Some("runner_error" | "failed")
        )
    {
        return None;
    }
    Some(json!({
        "reason": "spawn_terminal_failure",
        "source": "codex_control",
        "artifact_path": path,
        "failure": control,
    }))
}

fn read_spawned_agent_control_artifact(
    files: &AgentSpawnFiles,
    agent_kind: ActSpawnAgentCli,
) -> Result<Option<SpawnedAgentControlRead>, Value> {
    if agent_kind != ActSpawnAgentCli::Codex {
        return Ok(None);
    }
    let Some(path) = files.codex_app_server_control_path.as_ref() else {
        return Err(json!({ "reason": "codex_control_path_missing" }));
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(json!({
                "reason": "codex_control_artifact_missing",
                "control_path": path.display().to_string(),
            }));
        }
        Err(error) => {
            return Err(json!({
                "reason": "codex_control_artifact_read_failed",
                "control_path": path.display().to_string(),
                "error": error.to_string(),
            }));
        }
    };
    if bytes.is_empty() {
        return Err(json!({
            "reason": "codex_control_artifact_empty",
            "control_path": path.display().to_string(),
        }));
    }
    let json_bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    let control =
        serde_json::from_slice::<SpawnedAgentControlRead>(json_bytes).map_err(|error| {
            json!({
                "reason": "codex_control_artifact_json_invalid",
                "control_path": path.display().to_string(),
                "bytes": bytes.len(),
                "error": error.to_string(),
                "text": String::from_utf8_lossy(&bytes).into_owned(),
            })
        })?;
    let mut validation_errors = Vec::new();
    if control.schema_version != 1 {
        validation_errors.push("schema_version mismatch");
    }
    if control.protocol != "codex_app_server_ws" {
        validation_errors.push("protocol mismatch");
    }
    if !control.endpoint.starts_with("ws://127.0.0.1:") {
        validation_errors.push("endpoint must be loopback ws://127.0.0.1:<port>");
    }
    if control.control_path != path.display().to_string() {
        validation_errors.push("control_path mismatch");
    }
    if control.thread_id.as_deref().is_none_or(str::is_empty) {
        validation_errors.push("thread_id missing");
    }
    if control.turn_id.as_deref().is_none_or(str::is_empty) {
        validation_errors.push("turn_id missing");
    }
    if control.app_server_process_id == 0 {
        validation_errors.push("app_server_process_id missing");
    }
    if matches!(control.turn_status.as_str(), "runner_error" | "failed") {
        validation_errors.push("runner reported terminal failure");
    }
    if !validation_errors.is_empty() {
        return Err(json!({
            "reason": "codex_control_artifact_invalid",
            "control_path": path.display().to_string(),
            "validation_errors": validation_errors,
            "control": control,
        }));
    }
    Ok(Some(control))
}

fn read_json_file_lossy(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn agent_spawn_readiness_file_readback(files: &AgentSpawnFiles) -> Value {
    let terminal = agent_spawn_terminal_capture_artifacts(&files.log_dir);
    json!({
        "task_started_path": files.task_started_path.display().to_string(),
        "task_started_bytes": file_len(&files.task_started_path),
        "task_started": read_json_file_lossy(&files.task_started_path),
        "completion_status_path": files.completion_status_path.display().to_string(),
        "completion_status_bytes": file_len(&files.completion_status_path),
        "completion_status": read_json_file_lossy(&files.completion_status_path),
        "stdout_path": files.stdout_path.display().to_string(),
        "stdout_bytes": file_len(&files.stdout_path),
        "stderr_path": files.stderr_path.display().to_string(),
        "stderr_bytes": file_len(&files.stderr_path),
        "final_message_path": files.final_message_path.display().to_string(),
        "final_message_bytes": file_len(&files.final_message_path),
        "codex_app_server_control_path": files
            .codex_app_server_control_path
            .as_ref()
            .map(|path| path.display().to_string()),
        "codex_app_server_control_bytes": files
            .codex_app_server_control_path
            .as_ref()
            .map_or(0, |path| file_len(path)),
        "codex_app_server_control": files
            .codex_app_server_control_path
            .as_ref()
            .and_then(|path| read_json_file_lossy(path)),
        "codex_app_server_events_path": files
            .codex_app_server_events_path
            .as_ref()
            .map(|path| path.display().to_string()),
        "codex_app_server_events_bytes": files
            .codex_app_server_events_path
            .as_ref()
            .map_or(0, |path| file_len(path)),
        "terminal_asciicast_path": terminal.asciicast_path.display().to_string(),
        "terminal_asciicast_bytes": file_len(&terminal.asciicast_path),
        "terminal_capture_status_path": terminal.status_path.display().to_string(),
        "terminal_capture_status_bytes": file_len(&terminal.status_path),
        "terminal_capture_status": read_json_file_lossy(&terminal.status_path),
        "terminal_final_screen_path": terminal.final_screen_path.display().to_string(),
        "terminal_final_screen_bytes": file_len(&terminal.final_screen_path),
        "terminal_input_audit_path": terminal.input_audit_path.display().to_string(),
        "terminal_input_audit_bytes": file_len(&terminal.input_audit_path),
    })
}

#[derive(Clone, Debug)]
struct SpawnSessionCandidateRead {
    registry: SessionRegistryRead,
    active_target: Option<TargetWire>,
}

fn spawn_target_wire_from_session_target(target: &SessionTarget) -> TargetWire {
    match target {
        SessionTarget::Window { hwnd } => TargetWire::Window { window_hwnd: *hwnd },
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => TargetWire::Cdp {
            window_hwnd: *window_hwnd,
            cdp_target_id: cdp_target_id.clone(),
        },
    }
}

fn spawn_session_observation_from_read(
    candidate: &SpawnSessionCandidateRead,
    readiness: Value,
) -> Value {
    json!({
        "session_id": candidate.registry.session_id,
        "agent_kind": candidate.registry.agent_kind,
        "client_name": candidate.registry.client_name,
        "client_version": candidate.registry.client_version,
        "lifecycle": candidate.registry.lifecycle,
        "started_at_unix_ms": candidate.registry.started_at_unix_ms,
        "last_seen_unix_ms": candidate.registry.last_seen_unix_ms,
        "last_seen_ms_ago": candidate.registry.last_seen_ms_ago,
        "last_action": candidate.registry.last_action,
        "active_target": candidate.active_target,
        "readiness": readiness,
    })
}

fn spawn_session_candidate_readiness_from_read(
    registry: &SessionRegistryRead,
    active_target: Option<&TargetWire>,
    agent_kind: ActSpawnAgentCli,
    target: Option<&ActSpawnAgentTarget>,
    before_session_ids: &BTreeSet<String>,
    launched_at_unix_ms: u64,
) -> Value {
    if before_session_ids.contains(&registry.session_id) {
        return json!({
            "ready": false,
            "reason": "session_existed_before_spawn",
            "expected": "new distinct MCP session id",
        });
    }
    if registry.lifecycle != "live" {
        return json!({
            "ready": false,
            "reason": "session_not_live",
            "lifecycle": registry.lifecycle,
            "closed_at_unix_ms": registry.closed_at_unix_ms,
            "last_reason_code": registry.last_reason_code,
        });
    }
    if registry.started_at_unix_ms + 2_000 < launched_at_unix_ms {
        return json!({
            "ready": false,
            "reason": "session_started_before_spawn_window",
            "started_at_unix_ms": registry.started_at_unix_ms,
            "launched_at_unix_ms": launched_at_unix_ms,
            "allowed_clock_skew_ms": 2000,
        });
    }
    if !registry_matches_cli(registry, agent_kind) {
        return json!({
            "ready": false,
            "reason": "session_cli_mismatch",
            "expected_cli": agent_kind.as_str(),
            "agent_kind": registry.agent_kind,
            "client_name": registry.client_name,
        });
    }
    if let Some(expected) = target {
        if matches_target_wire(active_target, expected) {
            json!({
                "ready": true,
                "reason": "target_bound",
            })
        } else {
            json!({
                "ready": false,
                "reason": "target_mismatch",
                "expected_target": expected,
                "active_target": active_target,
            })
        }
    } else if registry
        .last_action
        .as_deref()
        .is_some_and(|action| action.starts_with("tools/call:"))
    {
        json!({
            "ready": true,
            "reason": "tool_call_observed",
            "last_action": registry.last_action,
        })
    } else {
        json!({
            "ready": false,
            "reason": "tool_call_not_observed",
            "last_action": registry.last_action,
            "expected": "last_action beginning with tools/call:",
        })
    }
}

fn task_start_session_id_for_spawn(files: &AgentSpawnFiles, spawn_id: &str) -> Option<String> {
    let value = read_json_file_lossy(&files.task_started_path)?;
    let object_spawn_id = value.get("spawn_id").and_then(Value::as_str)?;
    if object_spawn_id != spawn_id {
        return None;
    }
    value
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
        .map(str::to_owned)
}

fn spawn_session_identity_matches_from_read(
    registry: &SessionRegistryRead,
    agent_kind: ActSpawnAgentCli,
    before_session_ids: &BTreeSet<String>,
    launched_at_unix_ms: u64,
) -> bool {
    !before_session_ids.contains(&registry.session_id)
        && registry.lifecycle == "live"
        && registry.started_at_unix_ms + 2_000 >= launched_at_unix_ms
        && registry_matches_cli(registry, agent_kind)
}

fn registry_matches_cli(registry: &SessionRegistryRead, cli: ActSpawnAgentCli) -> bool {
    let cli = cli.as_str();
    if registry.agent_kind == cli {
        return true;
    }
    registry
        .client_name
        .as_deref()
        .is_some_and(|name| name.to_ascii_lowercase().contains(cli))
}

fn matches_target_wire(wire: Option<&super::TargetWire>, expected: &ActSpawnAgentTarget) -> bool {
    match (wire, expected) {
        (
            Some(super::TargetWire::Window {
                window_hwnd: actual,
            }),
            ActSpawnAgentTarget::Window {
                window_hwnd: expected,
            },
        ) => actual == expected,
        (
            Some(super::TargetWire::Cdp {
                window_hwnd: actual_hwnd,
                cdp_target_id: actual_target,
            }),
            ActSpawnAgentTarget::Cdp {
                window_hwnd: expected_hwnd,
                cdp_target_id: expected_target,
            },
        ) => actual_hwnd == expected_hwnd && actual_target == expected_target,
        _ => false,
    }
}

fn tail_file_lossy(path: &Path, limit_bytes: usize) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let start = bytes.len().saturating_sub(limit_bytes);
    Some(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

fn public_agent_log_tail(path: &Path, limit_bytes: usize) -> Option<String> {
    const REDACTION_CONTEXT_BYTES: usize = 4 * 1024;
    if limit_bytes == 0 {
        return Some(String::new());
    }
    let window_bytes = limit_bytes.saturating_add(REDACTION_CONTEXT_BYTES);
    let raw = tail_file_lossy(path, window_bytes)?;
    let truncated = fs::metadata(path)
        .is_ok_and(|metadata| metadata.len() > u64::try_from(window_bytes).unwrap_or(u64::MAX));
    let redacted = redact_log_window(&raw, truncated);
    if redacted.len() <= limit_bytes {
        return Some(redacted);
    }
    let mut start = redacted.len().saturating_sub(limit_bytes);
    while start < redacted.len() && !redacted.is_char_boundary(start) {
        start += 1;
    }
    Some(redacted[start..].to_owned())
}

fn process_has_exited(pid: u32) -> bool {
    !process_exists(pid)
}

fn process_exists(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let mut system = System::new();
    let pid = Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
    system.process(pid).is_some()
}

fn discover_agent_process_id(launcher_pid: u32, cli: ActSpawnAgentCli) -> Option<u32> {
    use sysinfo::{ProcessesToUpdate, System};

    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let descendants = descendant_process_ids(&system, launcher_pid);
    let cli = cli.as_str();
    descendants
        .iter()
        .copied()
        .find(|pid| {
            let Some(process) = system.process(sysinfo::Pid::from_u32(*pid)) else {
                return false;
            };
            let name = process.name().to_string_lossy().to_ascii_lowercase();
            let cmd = process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            name.contains(cli) || cmd.contains(cli)
        })
        .or_else(|| descendants.first().copied())
}

fn descendant_process_ids(system: &sysinfo::System, root_pid: u32) -> Vec<u32> {
    let mut descendants = Vec::new();
    let mut stack = vec![root_pid];
    while let Some(parent) = stack.pop() {
        for (pid, process) in system.processes() {
            if process.parent().map(|value| value.as_u32()) == Some(parent) {
                let child = pid.as_u32();
                descendants.push(child);
                stack.push(child);
            }
        }
    }
    descendants
}

fn agent_spawn_tool_error(
    code: &'static str,
    message: &'static str,
    data: serde_json::Value,
) -> ErrorData {
    tracing::warn!(code, "M4 agent spawn tool error: {message}");
    ErrorData::new(ErrorCode(-32099), message, Some(data))
}

fn launch_lifecycle_tool_error(message: &'static str, data: serde_json::Value) -> ErrorData {
    tracing::warn!(
        code = error_codes::TOOL_INTERNAL_ERROR,
        "M4 launch lifecycle tool error: {message}"
    );
    ErrorData::new(ErrorCode(-32099), message, Some(data))
}

async fn run_shell_with_idempotency(
    service: &SynapseService,
    params: ActRunShellParams,
    authorization: RunShellAuthorization,
    inline_await_limit_ms: u64,
    context: Option<&ShellExecutionContext>,
) -> Result<ActRunShellResponse, ErrorData> {
    validate_run_shell_execution_plan(&params, inline_await_limit_ms)?;
    let session_id = context.map(ShellExecutionContext::session_id);
    let Some(row_key) = run_shell_idempotency_row_key(&params, session_id)? else {
        return run_authorized_shell(params, &authorization, inline_await_limit_ms, context).await;
    };

    let runtime = service.reflex_runtime()?;
    let _live_reservation = {
        let runtime = runtime.lock().map_err(|_error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                "reflex runtime lock poisoned while checking act_run_shell idempotency",
            )
        })?;
        // A miss under the restart-stable global key may still be a legacy
        // session-scoped reservation persisted by a pre-#134 daemon that
        // crashed before writing its completion row; adopting it here keeps
        // the retry on the replay/recovery path instead of executing twice.
        let mut existing_row: Option<Vec<u8>> = runtime
            .storage_kv_row(&row_key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let mut legacy_source_key: Option<Vec<u8>> = None;
        if existing_row.is_none() {
            // HTTP sessions never survive a daemon restart, so legacy rows
            // are matched by a bounded scan rather than reconstructed keys.
            // The window starts at `session/` so accumulated `global/` rows
            // (which have no retention path) can never consume the budget:
            // `global/` sorts before `session/`, which sorts before
            // `sessionless/`. The window never materializes the whole
            // prefix, and a true legacy overflow (cap+1 rows) fails closed
            // instead of risking a double execution through an unscanned
            // legacy row.
            let legacy_scan_from = format!("{RUN_SHELL_IDEMPOTENCY_PREFIX}session/").into_bytes();
            let (window_rows, _more) = runtime
                .storage_cf_rows_from(
                    cf::CF_KV,
                    &legacy_scan_from,
                    RUN_SHELL_IDEMPOTENCY_LEGACY_SCAN_LIMIT + 1,
                )
                .map_err(|error| mcp_error(error.code(), error.to_string()))?;
            let scan_rows: Vec<(Vec<u8>, Vec<u8>)> = window_rows
                .into_iter()
                .filter(|(row_key, _)| row_key.starts_with(RUN_SHELL_IDEMPOTENCY_PREFIX.as_bytes()))
                .collect();
            if scan_rows.len() > RUN_SHELL_IDEMPOTENCY_LEGACY_SCAN_LIMIT {
                return Err(mcp_error(
                    synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                    "act_run_shell idempotency legacy scan cap reached; refusing to treat the key as unused because an unscanned legacy row could double-execute; purge completed legacy idempotency rows and retry",
                ));
            }
            if let Some((legacy_key, row_bytes)) =
                run_shell_idempotency_select_legacy_row(&params, &scan_rows)
            {
                existing_row = Some(row_bytes);
                legacy_source_key = Some(legacy_key);
            }
        }
        if let Some(existing) = existing_row {
            if let Some(legacy_key) = legacy_source_key {
                // Migrate before any replay validation can abort the call:
                // re-keying the legacy row under the restart-stable global
                // key in one flushed batch means a crash can never leave both
                // rows behind, and every future retry finds the row by the
                // exact global read instead of rescanning. The one-row batch
                // is correctness-critical metadata, so it takes the same
                // bounded pressure bypass the lease/profile recovery uses.
                runtime
                    .storage_replace_rows(
                        cf::CF_KV,
                        vec![legacy_key],
                        vec![(row_key.clone(), existing.clone())],
                    )
                    .map_err(|error| mcp_error(error.code(), error.to_string()))?;
            }
            let current_owner_is_live = LiveRunShellReservation::is_live(&row_key)?;
            if let Some(recovered) = run_shell_idempotency_recover_after_restart(
                &params,
                &existing,
                session_id,
                &RUN_SHELL_RESERVATION_OWNER_ID,
                current_owner_is_live,
            )? {
                runtime
                    .storage_put_kv_rows(vec![(row_key.clone(), recovered.clone())])
                    .map_err(|error| mcp_error(error.code(), error.to_string()))?;
                drop(runtime);
                return run_shell_idempotency_replay(&params, &recovered, session_id);
            }
            drop(runtime);
            return run_shell_idempotency_replay(&params, &existing, session_id);
        }
        let reservation = run_shell_idempotency_reservation_row(
            &params,
            &authorization,
            session_id,
            &RUN_SHELL_RESERVATION_OWNER_ID,
        )?;
        runtime
            .storage_put_kv_rows(vec![(row_key.clone(), reservation)])
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        LiveRunShellReservation::register(row_key.clone())?
    };

    let response = run_authorized_shell(
        params.clone(),
        &authorization,
        inline_await_limit_ms,
        context,
    )
    .await?;
    let completed =
        run_shell_idempotency_completed_row(&params, &authorization, &response, session_id)?;
    {
        let runtime = runtime.lock().map_err(|_error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                "reflex runtime lock poisoned while recording act_run_shell idempotency",
            )
        })?;
        runtime
            .storage_put_kv_rows(vec![(row_key, completed)])
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    }
    Ok(response)
}

#[cfg(test)]
#[path = "tests/spawn_terminal_failure.rs"]
mod spawn_terminal_failure_tests;

#[cfg(test)]
#[path = "tests/model_preflight_25.rs"]
mod model_preflight_tests;

#[cfg(test)]
#[path = "tests/spawn_model_entry_25.rs"]
mod spawn_model_entry_25_tests;

#[cfg(test)]
#[path = "tests/shell_idempotency_entry_134.rs"]
mod shell_idempotency_entry_134_tests;

#[cfg(test)]
#[path = "tests/claude_final_message_155.rs"]
mod claude_final_message_155_tests;

#[cfg(test)]
#[path = "tests/claude_result_is_error_155.rs"]
mod claude_result_is_error_tests;

#[cfg(test)]
#[path = "tests/started_rows_storage_198.rs"]
mod started_rows_storage_198;
