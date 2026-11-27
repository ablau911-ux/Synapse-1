// Split-out sibling of the m4_tools module; the glob mirrors the pre-split
// single-module layout and keeps the shared symbol set in one place.
#[allow(clippy::wildcard_imports)]
use super::*;

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    #[schemars(
        default,
        description = "Executable arguments for run/start. Explicit shells parse their args; cmd.exe operators and environment expansion are refused."
    )]
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
}

pub(crate) fn shell_input_schema() -> Arc<Map<String, Value>> {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["run", "start", "status", "cancel"],
                "default": "run",
                "description": "Shell facade operation. Omit only for the default run operation."
            },
            "command": {
                "type": ["string", "null"],
                "description": "Executable path/name only. Accepted by run/start."
            },
            "args": {
                "type": ["array", "null"],
                "items": { "type": "string" },
                "description": "Executable arguments for run/start. Explicit shells parse their args; cmd.exe operators and environment expansion are refused."
            },
            "working_dir": {
                "type": ["string", "null"],
                "description": "Working directory for run/start."
            },
            "env": {
                "type": ["object", "null"],
                "additionalProperties": { "type": "string" },
                "description": "Extra environment variables for run/start."
            },
            "timeout_ms": {
                "type": ["integer", "null"],
                "minimum": 1,
                "description": "run: caller inline wait budget. start: durable job lifetime cap. Omit start timeout for an unbounded durable job."
            },
            "execution_mode": {
                "type": ["string", "null"],
                "enum": ["auto", "inline", "durable", null],
                "description": "run only. Controls inline vs durable/background routing."
            },
            "durable_timeout_ms": {
                "type": ["integer", "null"],
                "minimum": 1,
                "description": "run only. Applies if run creates a durable/background job; start uses timeout_ms for its durable lifetime cap."
            },
            "idempotency_key": {
                "type": ["string", "null"],
                "description": "run only. Deduplicates/replays matching run requests."
            },
            "job_id": {
                "type": ["string", "null"],
                "minLength": 1,
                "maxLength": 128,
                "description": "start/status/cancel durable job id. Optional for start, required for status/cancel."
            },
            "tail_bytes": {
                "type": ["integer", "null"],
                "minimum": 0,
                "maximum": 1048576,
                "description": "status only. Number of stdout/stderr tail bytes to read."
            }
        },
        "oneOf": [
            {
                "title": "shell operation=run",
                "type": "object",
                "additionalProperties": false,
                "required": ["command"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "run",
                        "default": "run",
                        "description": "Default shell operation."
                    },
                    "command": { "$ref": "#/properties/command" },
                    "args": { "$ref": "#/properties/args" },
                    "working_dir": { "$ref": "#/properties/working_dir" },
                    "env": { "$ref": "#/properties/env" },
                    "timeout_ms": { "$ref": "#/properties/timeout_ms" },
                    "execution_mode": { "$ref": "#/properties/execution_mode" },
                    "durable_timeout_ms": { "$ref": "#/properties/durable_timeout_ms" },
                    "idempotency_key": { "$ref": "#/properties/idempotency_key" }
                }
            },
            {
                "title": "shell operation=start",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation", "command"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "start",
                        "description": "Create a durable shell job immediately."
                    },
                    "command": { "$ref": "#/properties/command" },
                    "args": { "$ref": "#/properties/args" },
                    "working_dir": { "$ref": "#/properties/working_dir" },
                    "env": { "$ref": "#/properties/env" },
                    "timeout_ms": { "$ref": "#/properties/timeout_ms" },
                    "job_id": { "$ref": "#/properties/job_id" }
                }
            },
            {
                "title": "shell operation=status",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation", "job_id"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "status",
                        "description": "Read persisted durable shell job state."
                    },
                    "job_id": { "$ref": "#/properties/job_id" },
                    "tail_bytes": { "$ref": "#/properties/tail_bytes" }
                }
            },
            {
                "title": "shell operation=cancel",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation", "job_id"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "cancel",
                        "description": "Terminate an exact durable shell job."
                    },
                    "job_id": { "$ref": "#/properties/job_id" }
                }
            }
        ]
    });
    match schema {
        Value::Object(object) => Arc::new(object),
        _ => Arc::new(Map::new()),
    }
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
    Terminate,
}

impl ProcessOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Launch => "launch",
            Self::History => "history",
            Self::Terminate => "terminate",
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
    #[schemars(
        range(min = 1),
        description = "launch: window/readiness wait budget. terminate: graceful-exit wait after WM_CLOSE before a forced kill (default 5000, max 30000)."
    )]
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
    #[schemars(
        description = "Exact pid filter for list/history. Required for terminate: the pid must be one this MCP session launched through process launch; any other pid is refused."
    )]
    pub pid: Option<u32>,
    #[serde(default)]
    pub process_name_contains: Option<String>,
    #[serde(default)]
    pub command_line_contains: Option<String>,
    #[serde(default)]
    #[schemars(
        range(min = 1, max = 1000),
        description = "Row limit for list (1..=1000, default 100) or history (1..=200, default 20)."
    )]
    pub limit: Option<usize>,
    #[serde(default)]
    #[schemars(default)]
    pub include_command_line: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<ProcessTerminateResponse>,
}

/// `process terminate` readback (#333-J). Only a pid this MCP session launched
/// through `process launch` is ever terminated; the session process-resource
/// ledger is the ownership authority and the launch-time Windows job object is
/// the ownership proof every destructive step is bound to.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessTerminateResponse {
    pub pid: u32,
    pub source_of_truth: String,
    pub ownership: String,
    pub launch_target: String,
    pub registered_at_unix_ms: u64,
    /// Process ids in the launch's Windows job object, read from the job
    /// before any close request (includes children whose root has exited).
    pub process_ids_before: Vec<u32>,
    /// Job members proven live and pinned by an open process handle before
    /// any close request.
    pub live_process_ids_before: Vec<u32>,
    /// Visible top-level windows of pinned job members that were sent
    /// `WM_CLOSE`.
    pub close_requested_hwnds: Vec<i64>,
    pub close_request_errors: Vec<String>,
    pub graceful_wait_ms: u64,
    pub exited_gracefully: bool,
    /// Present only when the job was not empty after the graceful step; the
    /// force step is `TerminateJobObject` on the launch job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_termination: Option<crate::m4::OwnedProcessTerminationReadback>,
    /// Process ids still in the launch job after the last step.
    pub remaining_process_ids: Vec<u32>,
    /// Live process count read from the launch job after the last step; the
    /// Source of Truth for `terminated` (`None` when it could not be read).
    pub job_active_processes_after: Option<u32>,
    /// Job readback failures (membership list or active count).
    pub job_readback_errors: Vec<String>,
    pub terminated: bool,
    /// True when the session ledger entry was released after the job was
    /// read back empty (an entry holding a hidden-desktop lease stays for
    /// session teardown).
    pub ledger_released: bool,
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
