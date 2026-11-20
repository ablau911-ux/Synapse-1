//! Authoritative session-scoped resource teardown (#801).
//!
//! Every resource that can outlive an MCP request should either be owned by an
//! explicit handle or be reclaimed here when the owning `Mcp-Session-Id` ends.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rmcp::{ErrorData, model::ErrorCode};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use synapse_action::ActionHandle;
use synapse_core::error_codes;
use synapse_storage::{Db, cf};

use crate::{
    http::sse::SseState,
    m1::mcp_error,
    m2::SharedSessionClipboardBuffers,
    m3::SharedM3State,
    m4::{self, OwnedProcessJob},
};

use super::{
    CdpTargetOwner, SessionTarget, SharedCdpTargetOwners, SharedLeaseTransitionGate,
    SharedSessionTargets, SynapseService,
    session_registry::{SharedSessionRegistry, unix_time_ms_now},
    target_claims::{self, SharedTargetClaims, TargetClaimCleanupReport},
    url_redaction::redact_url_for_public_readback,
};

const MCP_SESSION_STORE_PREFIX: &str = "mcp/session/v1/";
const PROCESS_JOB_CLOSE_WAIT: Duration = Duration::from_secs(5);
const ACT_SPAWN_AGENT_TOOL_NAME: &str = "act_spawn_agent";
const AGENT_SPAWN_COMPLETION_GRACE: Duration = Duration::from_secs(30);
const DAEMON_RESTART_BROWSER_CONTINUITY_GRACE: Duration = Duration::from_mins(30);
pub(crate) const HTTP_STALE_REASON: &str = "http_stale";
pub(crate) const SPAWN_COMPLETED_REASON: &str = "spawn_completed";
pub(crate) const SPAWNED_AGENT_PROCESS_EXITED_REASON: &str = "spawned_agent_process_exited";

pub(crate) type SharedSessionProcessResources =
    Arc<Mutex<BTreeMap<String, BTreeMap<u32, SessionProcessResource>>>>;
pub(crate) type SharedTerminatedSessions =
    Arc<Mutex<super::terminated_sessions::TerminatedSessions>>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionTeardownOptions {
    agent_spawn_completion_grace: Duration,
}

impl SessionTeardownOptions {
    pub(crate) const fn explicit_kill() -> Self {
        Self {
            agent_spawn_completion_grace: Duration::ZERO,
        }
    }
}

impl Default for SessionTeardownOptions {
    fn default() -> Self {
        Self {
            agent_spawn_completion_grace: AGENT_SPAWN_COMPLETION_GRACE,
        }
    }
}

pub(crate) fn mcp_session_store_key(session_id: &str) -> Vec<u8> {
    format!("{MCP_SESSION_STORE_PREFIX}{session_id}").into_bytes()
}

static SESSION_STORE_ROW_GATE: Mutex<()> = Mutex::new(());

/// #215: serializes read-modify-write and delete of persisted MCP session rows
/// (`mcp/session/v1/*` in `CF_KV`). The HTTP session store's activity refresh
/// and every row delete take it, so a refresh can never resurrect a row that
/// teardown just deleted. Hold it only across synchronous storage calls.
pub(crate) fn lock_session_store_rows() -> std::sync::MutexGuard<'static, ()> {
    SESSION_STORE_ROW_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug)]
pub(crate) struct SessionProcessResource {
    pub session_id: String,
    pub tool: &'static str,
    pub pid: u32,
    pub registered_at_unix_ms: u64,
    pub resource_id: Option<String>,
    pub launch_target: String,
    pub agent_cli: Option<String>,
    pub process_job: Option<OwnedProcessJob>,
    pub desktop_lease: Option<m4::LaunchDesktopLease>,
    /// #75: processes (pid -> start time) a failed teardown could not remove;
    /// the next attempt re-verifies and re-terminates them even when the root
    /// pid is already gone, so a retry never starts from an empty ownership set.
    pub retained_processes: BTreeMap<u32, u64>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionHiddenDesktopReadback {
    pub session_id: String,
    pub desktop_names: Vec<String>,
    pub launch_pids: Vec<u32>,
    pub resource_count: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCdpTargetOwnerReadback {
    pub owner_key: String,
    pub session_id: String,
    pub window_hwnd: i64,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub requested_url: String,
    pub target_url: String,
    pub created_at_unix_ms: u64,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionTargetShutdownReadback {
    Window {
        hwnd: i64,
    },
    Cdp {
        window_hwnd: i64,
        cdp_target_id: String,
    },
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionShutdownBrowserContinuityReport {
    pub source_of_truth: String,
    pub recovery_action: String,
    pub restart_grace_remaining_ms: u64,
    pub memory_target_before: Option<SessionTargetShutdownReadback>,
    pub persisted_target_before: Option<SessionTargetShutdownReadback>,
    pub memory_cdp_owner_count_before: usize,
    pub persisted_cdp_owner_count_before: usize,
    pub cdp_target_owners: Vec<SessionCdpTargetOwnerReadback>,
    pub tab_close_attempted: bool,
    pub tab_close_succeeded: bool,
    pub failed: bool,
    pub error_message: Option<String>,
}

impl SessionProcessResource {
    pub(crate) fn new(
        session_id: String,
        tool: &'static str,
        pid: u32,
        resource_id: Option<String>,
        launch_target: String,
        process_job: OwnedProcessJob,
    ) -> Self {
        Self {
            session_id,
            tool,
            pid,
            registered_at_unix_ms: unix_time_ms_now(),
            resource_id,
            launch_target,
            agent_cli: None,
            process_job: Some(process_job),
            desktop_lease: None,
            retained_processes: BTreeMap::new(),
        }
    }

    pub(crate) fn with_agent_cli(mut self, agent_cli: &str) -> Self {
        self.agent_cli = Some(agent_cli.to_owned());
        self
    }

    pub(crate) fn with_desktop_lease(
        mut self,
        desktop_lease: Option<m4::LaunchDesktopLease>,
    ) -> Self {
        self.desktop_lease = desktop_lease;
        self
    }
}

#[derive(Clone)]
pub(crate) struct SessionLifecycleState {
    started_at: Instant,
    action_handle: ActionHandle,
    m3_state: SharedM3State,
    session_targets: SharedSessionTargets,
    cdp_target_owners: SharedCdpTargetOwners,
    session_clipboards: SharedSessionClipboardBuffers,
    session_registry: SharedSessionRegistry,
    target_claims: SharedTargetClaims,
    session_processes: SharedSessionProcessResources,
    terminated_sessions: SharedTerminatedSessions,
    lease_transition_gate: SharedLeaseTransitionGate,
    seat_supervisor: Arc<synapse_seats::SeatSupervisor>,
    sse_state: SseState,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionTeardownReport {
    pub session_id: String,
    pub reason: String,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: u64,
    pub already_terminated: bool,
    pub marked_terminated: bool,
    pub termination_marker_failed: bool,
    pub termination_marker_error_message: Option<String>,
    pub input: SessionInputCleanupReport,
    pub seats: SessionSeatCleanupReport,
    pub target: SessionTargetCleanupReport,
    pub continuity: SessionContinuityCleanupReport,
    pub audit_session: SessionAuditSessionCleanupReport,
    pub clipboard: SessionClipboardCleanupReport,
    pub cdp: SessionCdpCleanupReport,
    pub target_claims: TargetClaimCleanupReport,
    pub shell: SessionShellCleanupReport,
    pub processes: SessionProcessCleanupReport,
    pub subscriptions: SessionSubscriptionCleanupReport,
    pub session_store: SessionStoreCleanupReport,
    pub registry: SessionRegistryCleanupReport,
    pub failure_count: u32,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSeatCleanupReport {
    pub source_of_truth: &'static str,
    pub owned_before: usize,
    pub closed: usize,
    pub failed: usize,
    pub failures: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionInputCleanupReport {
    pub snapshot_read_before: bool,
    pub owned_inputs_before: usize,
    pub lease_held_before: bool,
    pub lease_owner_before: Option<String>,
    pub released_keys: u32,
    pub released_buttons: u32,
    pub neutralized_pads: u32,
    pub retained_shared_inputs: u32,
    pub lease_released: bool,
    pub expired_lease_cleanup_completed: bool,
    pub lease_row_existed_before: bool,
    pub lease_row_deleted: bool,
    pub lease_row_exists_after: bool,
    pub lease_release_event_written: bool,
    pub snapshot_read_after: bool,
    pub owned_inputs_after: usize,
    pub lease_held_after: bool,
    pub lease_owner_after: Option<String>,
    pub failed: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "shutdown cleanup reports exact before/after lease row booleans"
)]
pub struct SessionShutdownInputCleanupReport {
    pub session_id: String,
    pub reason: String,
    pub process_jobs: SessionProcessRestartHandoffReport,
    pub browser_continuity: SessionShutdownBrowserContinuityReport,
    pub input: SessionInputCleanupReport,
    pub lease_row_existed_before: bool,
    pub lease_row_deleted: bool,
    pub lease_row_exists_after: bool,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionProcessRestartHandoffReport {
    pub owned_before: usize,
    pub job_handles_before: usize,
    pub kill_on_close_disarmed: usize,
    pub failed: usize,
    pub items: Vec<SessionProcessRestartHandoffItem>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionProcessRestartHandoffItem {
    pub tool: String,
    pub pid: u32,
    pub resource_id: Option<String>,
    pub launch_target: String,
    pub agent_cli: Option<String>,
    pub registered_at_unix_ms: u64,
    pub status: String,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionTargetCleanupReport {
    pub target_cleared: bool,
    pub target_sessions_before: usize,
    pub target_sessions_after: usize,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionContinuityCleanupReport {
    pub target_row_existed_before: bool,
    pub target_row_deleted: bool,
    pub target_row_exists_after: bool,
    pub lease_row_existed_before: bool,
    pub lease_row_deleted: bool,
    pub lease_row_exists_after: bool,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionAuditSessionCleanupReport {
    pub cache_sessions_before: usize,
    pub cache_sessions_after: usize,
    pub removed: bool,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionClipboardCleanupReport {
    pub buffer_existed_before: bool,
    pub buffer_count_before: usize,
    pub removed: bool,
    pub buffer_exists_after: bool,
    pub buffer_count_after: usize,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCdpCleanupReport {
    pub memory_owned_before: usize,
    pub persisted_owned_before: usize,
    pub owned_before: usize,
    pub closed: usize,
    pub already_absent: usize,
    pub endpoint_unreachable: usize,
    pub persisted_rows_deleted: usize,
    pub failed: usize,
    pub target_ids: Vec<String>,
    pub error_messages: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionShellCleanupReport {
    pub job_root: Option<String>,
    pub status_files_read: usize,
    pub skipped_invalid_job_dirs: usize,
    pub skipped_unreadable_status_files: usize,
    pub skipped_foreign_jobs: usize,
    pub live_jobs_before: usize,
    pub retained_live_jobs: usize,
    pub termination_attempted: usize,
    pub termination_succeeded: usize,
    pub failed: usize,
    pub job_ids: Vec<String>,
    pub remaining_process_ids: Vec<u32>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionProcessCleanupReport {
    pub owned_before: usize,
    pub job_close_attempted: usize,
    pub force_termination_attempted: usize,
    pub terminated: usize,
    pub failed: usize,
    pub items: Vec<SessionProcessCleanupItem>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionProcessCleanupItem {
    pub tool: String,
    pub pid: u32,
    pub resource_id: Option<String>,
    pub launch_target: String,
    pub agent_cli: Option<String>,
    pub registered_at_unix_ms: u64,
    pub process_ids_before: Vec<u32>,
    pub live_process_ids_before: Vec<u32>,
    pub job_handle_dropped: bool,
    pub natural_exit_wait_ms: u64,
    pub force_termination_status: Option<String>,
    pub completion_status_path: Option<String>,
    pub completion_status_before_cleanup: Option<String>,
    pub completion_artifact_cleanup_status: Option<String>,
    pub completion_artifact_cleanup_error: Option<String>,
    pub desktop_name: Option<String>,
    pub desktop_close_attempted: bool,
    pub desktop_close_succeeded: Option<bool>,
    pub desktop_close_error: Option<String>,
    pub desktop_window_process_ids_before: Vec<u32>,
    pub desktop_window_termination_attempted: bool,
    pub desktop_window_termination_status: Option<String>,
    pub desktop_window_process_ids_after: Vec<u32>,
    pub remaining_process_ids_after: Vec<u32>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSubscriptionCleanupReport {
    pub owned_before: usize,
    pub cancelled: usize,
    pub subscription_ids: Vec<String>,
    pub failed: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionStoreCleanupReport {
    pub key: String,
    pub existed_before: bool,
    pub deleted: bool,
    pub exists_after: bool,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionRegistryCleanupReport {
    pub closed_recorded: bool,
    pub reason_code: String,
    /// Whether this teardown transitioned the registry entry to closed and
    /// therefore journaled the terminal `exited` agent event (#897).
    pub journal_event_written: bool,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Debug)]
struct AgentSpawnCleanupRead {
    spawn_id: String,
    log_dir: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    final_message_path: PathBuf,
    completion_status_path: PathBuf,
}

impl AgentSpawnCleanupRead {
    fn for_resource(resource: &SessionProcessResource) -> Option<Self> {
        if resource.tool != ACT_SPAWN_AGENT_TOOL_NAME {
            return None;
        }
        let spawn_id = resource.resource_id.as_ref()?;
        if !spawn_id.starts_with("agent-spawn-") {
            return None;
        }
        let local_appdata = std::env::var_os("LOCALAPPDATA")?;
        let log_dir = PathBuf::from(local_appdata)
            .join("Synapse")
            .join("agent-spawns")
            .join(spawn_id);
        Some(Self {
            spawn_id: spawn_id.clone(),
            stdout_path: log_dir.join("stdout.jsonl"),
            stderr_path: log_dir.join("stderr.log"),
            final_message_path: log_dir.join("final-message.txt"),
            completion_status_path: log_dir.join("completion-status.json"),
            log_dir,
        })
    }

    fn completion_status(&self) -> Option<String> {
        let bytes = fs::read(&self.completion_status_path).ok()?;
        let status = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
        status
            .get("status")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
    }

    fn has_terminal_completion_status(&self) -> bool {
        self.completion_status()
            .is_some_and(|status| status != "running")
    }

    fn write_terminal_artifact(
        &self,
        status: &str,
        resource: &SessionProcessResource,
        process_ids_before: &[u32],
        live_process_ids_before: &[u32],
        remaining_process_ids_after: &[u32],
        natural_exit_wait_ms: u64,
        job_close_terminated_processes: bool,
        force_termination_status: Option<&str>,
    ) -> Result<String, String> {
        if self.has_terminal_completion_status() {
            return Ok("already_terminal".to_owned());
        }
        let completion_status_before_cleanup = self.completion_status();
        let final_message_len_before = file_len(&self.final_message_path);
        let stdout_len = file_len(&self.stdout_path);
        let stderr_len = file_len(&self.stderr_path);
        let (stdout_line_count, last_stdout_event_type) = stdout_summary(&self.stdout_path);
        let recovered_from_final_message = final_message_len_before > 0
            && remaining_process_ids_after.is_empty()
            && !job_close_terminated_processes
            && force_termination_status.is_none();
        let artifact_status = if recovered_from_final_message {
            "ok"
        } else {
            status
        };
        let artifact_error_message = if recovered_from_final_message {
            serde_json::Value::Null
        } else {
            json!("spawned agent session ended before a terminal completion artifact was written")
        };
        let hold_open_elapsed_ms_met = if recovered_from_final_message {
            serde_json::Value::Null
        } else {
            json!(false)
        };
        let final_message_source = if recovered_from_final_message {
            "preexisting_final_message"
        } else if final_message_len_before > 0 {
            "preexisting_final_message_without_terminal_status"
        } else {
            "session_cleanup_artifact_json"
        };
        let details = json!({
            "reason": if recovered_from_final_message {
                "session_teardown_recovered_preexisting_final_message"
            } else {
                "session_teardown_before_spawn_completion_artifact"
            },
            "session_id": &resource.session_id,
            "tool": resource.tool,
            "pid": resource.pid,
            "resource_id": &resource.resource_id,
            "launch_target": &resource.launch_target,
            "agent_cli": &resource.agent_cli,
            "registered_at_unix_ms": resource.registered_at_unix_ms,
            "completion_status_before_cleanup": completion_status_before_cleanup,
            "completion_status_recovered_from_final_message": recovered_from_final_message,
            "process_ids_before": process_ids_before,
            "live_process_ids_before": live_process_ids_before,
            "remaining_process_ids_after": remaining_process_ids_after,
            "natural_exit_wait_ms": natural_exit_wait_ms,
            "job_close_terminated_processes": job_close_terminated_processes,
            "force_termination_status": force_termination_status,
        });
        if final_message_len_before == 0 {
            let final_message = json!({
                "schema_version": 1,
                "spawn_id": &self.spawn_id,
                "cli": resource.agent_cli.as_deref().unwrap_or("unknown"),
                "status": status,
                "exit_code": null,
                "error_message": "spawned agent session ended before a terminal completion artifact was written",
                "message": "Synapse session cleanup wrote this terminal artifact because the spawned process ended or was reclaimed without a final assistant response artifact.",
                "stdout_path": self.stdout_path.display().to_string(),
                "stderr_path": self.stderr_path.display().to_string(),
                "completion_status_path": self.completion_status_path.display().to_string(),
                "details": details,
            });
            let bytes = serde_json::to_vec_pretty(&final_message).map_err(|error| {
                format!("failed to encode agent spawn cleanup final-message artifact: {error}")
            })?;
            fs::write(&self.final_message_path, bytes).map_err(|error| {
                format!(
                    "failed to write agent spawn cleanup final-message artifact {}: {error}",
                    self.final_message_path.display()
                )
            })?;
        }
        let completed_at_unix_ms = unix_time_ms_now();
        let final_message_len_after = file_len(&self.final_message_path);
        let completion_status = json!({
            "schema_version": 1,
            "spawn_id": &self.spawn_id,
            "cli": resource.agent_cli.as_deref().unwrap_or("unknown"),
            "status": artifact_status,
            "exit_code": null,
            "error_message": artifact_error_message,
            "wrapper_started_at_unix_ms": null,
            "completed_at_unix_ms": completed_at_unix_ms,
            "elapsed_ms": null,
            "requested_hold_open_ms": null,
            "hold_open_elapsed_ms_met": hold_open_elapsed_ms_met,
            "final_message_path": self.final_message_path.display().to_string(),
            "final_message_bytes": final_message_len_after,
            "final_message_present": final_message_len_after > 0,
            "final_message_source": final_message_source,
            "recovered_final_message_written": false,
            "fallback_final_message_written": final_message_len_before == 0,
            "stdout_path": self.stdout_path.display().to_string(),
            "stdout_line_count": stdout_line_count,
            "last_stdout_event_type": last_stdout_event_type,
            "stdout_bytes": stdout_len,
            "stderr_path": self.stderr_path.display().to_string(),
            "stderr_bytes": stderr_len,
            "daemon_terminal_artifact": true,
            "session_cleanup_artifact": true,
            "completion_status_source": if recovered_from_final_message {
                "session_cleanup_recovered_from_final_message"
            } else {
                "session_cleanup_artifact_json"
            },
            "completion_status_before_cleanup": completion_status_before_cleanup,
            "completion_status_recovered_from_final_message": recovered_from_final_message,
            "log_dir": self.log_dir.display().to_string(),
            "details": details,
        });
        let bytes = serde_json::to_vec_pretty(&completion_status).map_err(|error| {
            format!("failed to encode agent spawn cleanup completion-status artifact: {error}")
        })?;
        fs::write(&self.completion_status_path, bytes).map_err(|error| {
            format!(
                "failed to write agent spawn cleanup completion-status artifact {}: {error}",
                self.completion_status_path.display()
            )
        })?;
        Ok(if recovered_from_final_message {
            "recovered_final_message".to_owned()
        } else {
            status.to_owned()
        })
    }
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// How much of the end of `stdout.jsonl` is parsed for the last event type.
const STDOUT_SUMMARY_TAIL_BYTES: u64 = 256 * 1024;

/// #212: streams the file in fixed-size chunks to count non-empty lines and
/// parses only the tail for the last event type, so a multi-GB `stdout.jsonl`
/// neither spikes memory nor is read twice.
pub(super) fn stdout_summary(path: &Path) -> (u64, Option<String>) {
    let Ok(mut file) = fs::File::open(path) else {
        return (0, None);
    };
    let Ok(line_count) = count_non_empty_lines(&mut file) else {
        return (0, None);
    };
    (line_count, last_event_type_in_tail(&mut file))
}

fn count_non_empty_lines(file: &mut fs::File) -> std::io::Result<u64> {
    use std::io::Read as _;

    let mut chunk = vec![0_u8; 64 * 1024];
    let mut count = 0_u64;
    let mut line_has_content = false;
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        for byte in &chunk[..read] {
            if *byte == b'\n' {
                if line_has_content {
                    count += 1;
                    line_has_content = false;
                }
            } else if !byte.is_ascii_whitespace() {
                line_has_content = true;
            }
        }
    }
    if line_has_content {
        count += 1;
    }
    Ok(count)
}

fn last_event_type_in_tail(file: &mut fs::File) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(STDOUT_SUMMARY_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.by_ref()
        .take(STDOUT_SUMMARY_TAIL_BYTES)
        .read_to_end(&mut tail)
        .ok()?;
    let text = String::from_utf8_lossy(&tail);
    let mut lines = text.lines();
    if start > 0 {
        // The window may begin mid-record; drop the partial first line.
        lines.next();
    }
    // Newest event first: stop at the first line that carries a type.
    lines
        .rev()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| {
            value
                .get("type")
                .and_then(|value| value.as_str())
                .or_else(|| {
                    value
                        .get("item")
                        .and_then(|item| item.get("type"))
                        .and_then(|value| value.as_str())
                })
                .map(ToOwned::to_owned)
        })
        .next()
}

impl SessionTeardownReport {
    fn new(session_id: &str, reason: &str) -> Self {
        let now = unix_time_ms_now();
        Self {
            session_id: session_id.to_owned(),
            reason: reason.to_owned(),
            started_at_unix_ms: now,
            finished_at_unix_ms: now,
            already_terminated: false,
            marked_terminated: false,
            termination_marker_failed: false,
            termination_marker_error_message: None,
            input: SessionInputCleanupReport::default(),
            seats: SessionSeatCleanupReport::default(),
            target: SessionTargetCleanupReport::default(),
            continuity: SessionContinuityCleanupReport::default(),
            audit_session: SessionAuditSessionCleanupReport::default(),
            clipboard: SessionClipboardCleanupReport::default(),
            cdp: SessionCdpCleanupReport::default(),
            target_claims: TargetClaimCleanupReport::default(),
            shell: SessionShellCleanupReport::default(),
            processes: SessionProcessCleanupReport::default(),
            subscriptions: SessionSubscriptionCleanupReport::default(),
            session_store: SessionStoreCleanupReport::default(),
            registry: SessionRegistryCleanupReport::default(),
            failure_count: 0,
        }
    }

    fn finalize(&mut self) {
        self.finished_at_unix_ms = unix_time_ms_now();
        self.failure_count = 0;
        if self.termination_marker_failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        if self.input.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        self.failure_count = self
            .failure_count
            .saturating_add(u32::try_from(self.seats.failed).unwrap_or(u32::MAX));
        if self.target.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        if self.continuity.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        if self.audit_session.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        if self.clipboard.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        if self.target_claims.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        self.failure_count = self
            .failure_count
            .saturating_add(u32::try_from(self.cdp.failed).unwrap_or(u32::MAX));
        self.failure_count = self
            .failure_count
            .saturating_add(u32::try_from(self.shell.failed).unwrap_or(u32::MAX));
        self.failure_count = self
            .failure_count
            .saturating_add(u32::try_from(self.processes.failed).unwrap_or(u32::MAX));
        if self.subscriptions.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        if self.session_store.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        if self.registry.failed {
            self.failure_count = self.failure_count.saturating_add(1);
        }
    }
}

impl SynapseService {
    pub(crate) fn session_lifecycle_state(&self) -> Result<SessionLifecycleState, ErrorData> {
        Ok(SessionLifecycleState {
            started_at: self.started_at,
            action_handle: self.unscoped_action_handle().map_err(|error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("read unscoped action handle for session lifecycle: {error}"),
                )
            })?,
            m3_state: self.m3_state_handle(),
            session_targets: Arc::clone(&self.session_targets),
            cdp_target_owners: Arc::clone(&self.cdp_target_owners),
            session_clipboards: Arc::clone(&self.session_clipboards),
            session_registry: Arc::clone(&self.session_registry),
            target_claims: Arc::clone(&self.target_claims),
            session_processes: Arc::clone(&self.session_processes),
            terminated_sessions: Arc::clone(&self.terminated_sessions),
            lease_transition_gate: self.lease_transition_gate(),
            seat_supervisor: self.seat_supervisor_handle(),
            sse_state: self.sse_state()?,
        })
    }

    pub(crate) fn register_session_process_resource(
        &self,
        resource: SessionProcessResource,
    ) -> Result<(), ErrorData> {
        let session_id = resource.session_id.clone();
        let pid = resource.pid;
        let mut guard = self.session_processes.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session process resource ledger lock poisoned",
            )
        })?;
        let processes = guard.entry(session_id.clone()).or_default();
        if processes.contains_key(&pid) {
            return Err(ErrorData::new(
                ErrorCode(-32099),
                format!("session process resource already registered for pid {pid}"),
                Some(json!({
                    "code": error_codes::TOOL_INTERNAL_ERROR,
                    "session_id": session_id,
                    "pid": pid,
                    "reason": "duplicate_session_process_resource",
                })),
            ));
        }
        processes.insert(pid, resource);
        tracing::info!(
            code = "MCP_SESSION_PROCESS_RESOURCE_REGISTERED",
            session_id,
            pid,
            "readback=session_process_ledger after=registered"
        );
        Ok(())
    }

    pub(crate) fn session_hidden_desktop_readback(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionHiddenDesktopReadback>, ErrorData> {
        let guard = self.session_processes.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session process resource lock poisoned while reading hidden desktop ownership",
            )
        })?;
        let Some(resources) = guard.get(session_id) else {
            return Ok(None);
        };
        let mut desktop_names = BTreeSet::new();
        let mut launch_pids = Vec::new();
        for resource in resources.values() {
            let Some(desktop) = resource.desktop_lease.as_ref() else {
                continue;
            };
            if !desktop.is_session_owned() {
                continue;
            }
            desktop_names.insert(desktop.name().to_owned());
            launch_pids.push(resource.pid);
        }
        if desktop_names.is_empty() {
            return Ok(None);
        }
        launch_pids.sort_unstable();
        launch_pids.dedup();
        Ok(Some(SessionHiddenDesktopReadback {
            session_id: session_id.to_owned(),
            desktop_names: desktop_names.into_iter().collect(),
            resource_count: launch_pids.len(),
            launch_pids,
        }))
    }

    pub(crate) fn hidden_desktop_readbacks(
        &self,
    ) -> Result<Vec<SessionHiddenDesktopReadback>, ErrorData> {
        let guard = self.session_processes.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session process resource lock poisoned while reading hidden desktop ownership",
            )
        })?;
        let mut rows = Vec::new();
        for (session_id, resources) in guard.iter() {
            let mut desktop_names = BTreeSet::new();
            let mut launch_pids = Vec::new();
            for resource in resources.values() {
                let Some(desktop) = resource.desktop_lease.as_ref() else {
                    continue;
                };
                if !desktop.is_session_owned() {
                    continue;
                }
                desktop_names.insert(desktop.name().to_owned());
                launch_pids.push(resource.pid);
            }
            if desktop_names.is_empty() {
                continue;
            }
            launch_pids.sort_unstable();
            launch_pids.dedup();
            rows.push(SessionHiddenDesktopReadback {
                session_id: session_id.clone(),
                desktop_names: desktop_names.into_iter().collect(),
                resource_count: launch_pids.len(),
                launch_pids,
            });
        }
        rows.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(rows)
    }

    pub(crate) fn cdp_target_owner_readbacks(
        &self,
    ) -> Result<Vec<SessionCdpTargetOwnerReadback>, ErrorData> {
        let guard = self.cdp_target_owners.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "CDP target ownership registry lock poisoned while reading dashboard snapshot",
            )
        })?;
        let mut rows = guard
            .iter()
            .map(|(owner_key, owner)| SessionCdpTargetOwnerReadback {
                owner_key: owner_key.clone(),
                session_id: owner.session_id.clone(),
                window_hwnd: owner.window_hwnd,
                endpoint: owner.endpoint.clone(),
                cdp_target_id: owner.cdp_target_id.clone(),
                requested_url: redact_url_for_public_readback(&owner.requested_url),
                target_url: redact_url_for_public_readback(&owner.target_url),
                created_at_unix_ms: owner.created_at_unix_ms,
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.session_id
                .cmp(&right.session_id)
                .then_with(|| left.owner_key.cmp(&right.owner_key))
        });
        Ok(rows)
    }

    pub(crate) fn terminated_sessions_handle(&self) -> SharedTerminatedSessions {
        Arc::clone(&self.terminated_sessions)
    }
}

/// #75: retry teardown ids. HTTP keep-alive membership suppresses a retry only
/// for non-spawn sessions; confirmed OS-dead spawned owners always retry (the
/// 30 s retry cadence in `failed_cleanup_retry_ids` bounds the rate).
pub(crate) fn retry_teardown_ids(
    retries: &BTreeSet<String>,
    dead_owner_retries: &BTreeSet<String>,
    http_active: &BTreeSet<String>,
) -> BTreeSet<String> {
    retries
        .iter()
        .filter(|id| dead_owner_retries.contains(*id) || !http_active.contains(*id))
        .cloned()
        .collect()
}

impl SessionLifecycleState {
    /// Periodic metadata retention; never terminates a process or deletes its
    /// durable history. Registry cadence limits this to one pass per 30 seconds.
    pub(crate) async fn failed_cleanup_retry_candidates(
        &self,
    ) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
        let (entries, due) = match self.session_registry.lock() {
            Ok(mut registry) => (
                registry.failed_cleanup_entries(),
                registry.failed_cleanup_retry_ids(unix_time_ms_now()),
            ),
            Err(_) => return (BTreeSet::new(), BTreeSet::new(), BTreeSet::new()),
        };
        // Failed cleanup must not bypass ownership protection through another
        // resource ledger on intervening maintenance ticks, including closed rows.
        let mut protected: BTreeSet<String> = entries
            .iter()
            .map(|entry| entry.session_id.clone())
            .collect();
        if due.is_empty() {
            return (BTreeSet::new(), BTreeSet::new(), protected);
        }
        let live_pids = tokio::task::spawn_blocking(|| {
            let mut system = sysinfo::System::new();
            if system.refresh_processes(sysinfo::ProcessesToUpdate::All, false) == 0
                || system
                    .process(sysinfo::Pid::from_u32(std::process::id()))
                    .is_none()
            {
                return None;
            }
            Some(
                system
                    .processes()
                    .keys()
                    .map(|pid| pid.as_u32())
                    .collect::<BTreeSet<_>>(),
            )
        })
        .await
        .ok()
        .flatten();
        let mut retries = BTreeSet::new();
        let mut dead_owner_retries = BTreeSet::new();
        for entry in entries {
            if !due.contains(&entry.session_id) {
                continue;
            }
            // #213: a teardown still running is in progress, not failed; the
            // session stays protected and is retried only once it finished.
            if self.session_teardown_in_progress(&entry.session_id) {
                continue;
            }
            if let Some(spawned) = &entry.spawned_agent {
                let mut pids = vec![spawned.launcher_process_id];
                pids.extend(spawned.agent_process_id);
                pids.extend(
                    spawned
                        .control
                        .as_ref()
                        .map(|control| control.app_server_process_id),
                );
                pids.retain(|pid| *pid != 0);
                if pids.is_empty()
                    || live_pids
                        .as_ref()
                        .is_none_or(|live| pids.iter().any(|pid| live.contains(pid)))
                {
                    continue;
                }
            }
            if self
                .session_registry
                .lock()
                .is_ok_and(|registry| registry.cleanup_retry_unchanged(&entry))
            {
                protected.remove(&entry.session_id);
                // #75: spawned owners reaching here are confirmed OS-dead.
                if entry.spawned_agent.is_some() {
                    dead_owner_retries.insert(entry.session_id.clone());
                }
                retries.insert(entry.session_id);
            }
        }
        (retries, dead_owner_retries, protected)
    }

    pub(crate) async fn prune_expired_registry_entries(
        &self,
        session_manager: &rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
        failed_cleanup_sessions: &BTreeSet<String>,
    ) {
        let candidates = match self.session_registry.lock() {
            Ok(mut registry) => registry.expired_retention_candidates(unix_time_ms_now()),
            Err(_) => return,
        };
        if candidates.is_empty() {
            return;
        }
        let has_spawned = candidates.iter().any(|entry| entry.spawned_agent.is_some());
        let live_pids = if has_spawned {
            tokio::task::spawn_blocking(|| {
                let mut system = sysinfo::System::new();
                let count = system.refresh_processes(sysinfo::ProcessesToUpdate::All, false);
                // A failed Windows Toolhelp snapshot returns zero. Access-denied
                // processes still have PID entries; never infer death from an
                // absent process handle/name. Our own PID must be observable.
                if count == 0
                    || system
                        .process(sysinfo::Pid::from_u32(std::process::id()))
                        .is_none()
                {
                    return None;
                }
                Some(
                    system
                        .processes()
                        .keys()
                        .map(|pid| pid.as_u32())
                        .collect::<BTreeSet<_>>(),
                )
            })
            .await
            .ok()
            .flatten()
        } else {
            None
        };
        // Hold current admission/ownership readbacks through the short removal
        // section. Revision comparison also catches restoration before rmcp has
        // installed the restored session in its in-memory map.
        let active = session_manager.sessions.read().await;
        let Ok(owned) = self.session_processes.lock() else {
            return;
        };
        let Ok(mut registry) = self.session_registry.lock() else {
            return;
        };
        let before = registry.retention_counts().0;
        let mut active_pinned = 0_u64;
        let mut cleanup_failed_pinned = 0_u64;
        let mut ownership_pinned = 0_u64;
        let mut changed = 0_u64;
        let mut pruned = 0_u64;
        let now = unix_time_ms_now();
        for candidate in &candidates {
            if candidate.cleanup_failed || failed_cleanup_sessions.contains(&candidate.session_id) {
                cleanup_failed_pinned += 1;
                continue;
            }
            if active.contains_key(candidate.session_id.as_str()) {
                active_pinned += 1;
                continue;
            }
            if owned
                .get(&candidate.session_id)
                .is_some_and(|jobs| !jobs.is_empty())
            {
                ownership_pinned += 1;
                continue;
            }
            if let Some(spawned) = &candidate.spawned_agent {
                let mut pids = vec![spawned.launcher_process_id];
                pids.extend(spawned.agent_process_id);
                pids.extend(
                    spawned
                        .control
                        .as_ref()
                        .map(|control| control.app_server_process_id),
                );
                pids.retain(|pid| *pid != 0);
                if pids.is_empty()
                    || live_pids
                        .as_ref()
                        .is_none_or(|live| pids.iter().any(|pid| live.contains(pid)))
                {
                    ownership_pinned += 1;
                    continue;
                }
            }
            if registry.prune_expired_candidate(candidate, now) {
                pruned += 1;
            } else {
                changed += 1;
            }
        }
        let (after, pruned_total) = registry.retention_counts();
        tracing::info!(
            code = "MCP_SESSION_REGISTRY_RETENTION",
            before,
            after,
            pruned,
            pruned_total,
            candidates = candidates.len(),
            active_pinned,
            ownership_pinned,
            cleanup_failed_pinned,
            changed,
            stale_after_ms = registry.stale_after_ms(),
            process_snapshot_available = live_pids.is_some(),
            "readback=session_registry after=expired_inactive_metadata_pruned"
        );
    }

    /// #213: whether a teardown of `session_id` is running or queued right now.
    pub(crate) fn session_teardown_in_progress(&self, session_id: &str) -> bool {
        teardown_gate::SessionTeardownGate::global().is_in_progress(session_id)
    }

    /// #214: the ids in `candidates` that session teardown already marked
    /// terminated (used to find terminated sessions still open in rmcp).
    pub(crate) fn terminated_among(&self, candidates: &BTreeSet<String>) -> Vec<String> {
        self.terminated_sessions.lock().map_or_else(
            |_poisoned| Vec::new(),
            |terminated| {
                candidates
                    .iter()
                    .filter(|session_id| terminated.contains(session_id.as_str()))
                    .cloned()
                    .collect()
            },
        )
    }

    pub(crate) fn is_session_terminated(&self, session_id: &str) -> bool {
        self.terminated_sessions
            .lock()
            .is_ok_and(|terminated| terminated.contains(session_id))
    }

    pub(crate) async fn teardown_session(
        &self,
        session_id: &str,
        reason: &str,
    ) -> Result<SessionTeardownReport, ErrorData> {
        self.teardown_session_with_options(session_id, reason, SessionTeardownOptions::default())
            .await
    }

    pub(crate) async fn teardown_session_with_options(
        &self,
        session_id: &str,
        reason: &str,
        options: SessionTeardownOptions,
    ) -> Result<SessionTeardownReport, ErrorData> {
        let report = self
            .teardown_session_with_options_report(session_id, reason, options)
            .await?;
        if report.failure_count == 0 {
            Ok(report)
        } else {
            Err(session_teardown_error(report))
        }
    }

    pub(crate) async fn teardown_session_with_options_report(
        &self,
        session_id: &str,
        reason: &str,
        options: SessionTeardownOptions,
    ) -> Result<SessionTeardownReport, ErrorData> {
        validate_lifecycle_session_id(session_id)?;
        // Pin existing metadata before the first await: cancellation or an
        // early error must not let a later retention pass forget this cleanup.
        if let Ok(mut registry) = self.session_registry.lock() {
            registry.record_cleanup_result(session_id, false);
        }
        // #213: one teardown per session at a time. A second caller (DELETE,
        // retry, sweep, agent_kill, adopt) waits here instead of racing the
        // first over the same owner rows; the permit drops on completion or
        // cancellation.
        // Shared with the blocking cleanup tasks: if this future is cancelled
        // (e.g. the stale sweep's 120 s timeout) while they still run, the
        // session stays "in progress" until they finish, so a retry cannot
        // start a concurrent second teardown.
        let teardown_permit = Arc::new(
            teardown_gate::SessionTeardownGate::global()
                .begin(session_id)
                .await,
        );
        let mut report = SessionTeardownReport::new(session_id, reason);
        self.mark_terminated_session(&mut report);
        // #22: a spawned agent's scoped credential dies with its session, before
        // any later cleanup step can fail.
        if let Some(spawn_id) = crate::spawn_credentials::registry().revoke_session(session_id) {
            tracing::info!(
                code = "SPAWN_CREDENTIAL_REVOKED",
                session_id,
                spawn_id = %spawn_id,
                reason,
                "scoped spawn credential revoked at session teardown"
            );
        }
        report.input = self
            .cleanup_inputs_and_lease(session_id, "session_teardown")
            .await;
        report.seats = self.cleanup_seats(session_id).await;
        report.target = self.cleanup_target(session_id);
        report.continuity = self.cleanup_continuity(session_id);
        report.audit_session = self.cleanup_audit_session(session_id);
        report.clipboard = self.cleanup_clipboard(session_id);
        report.cdp =
            cleanup_session_cdp_targets(&self.m3_state, &self.cdp_target_owners, session_id).await;
        report.target_claims = self.cleanup_target_claims(session_id);
        report.shell =
            cleanup_shell_jobs_off_worker(session_id, reason, Arc::clone(&teardown_permit)).await;
        report.processes = self
            .cleanup_owned_processes_off_worker(session_id, options, Arc::clone(&teardown_permit))
            .await;
        let registry_reason = reconciled_teardown_exit_reason(reason, &report.processes);
        report.reason.clone_from(&registry_reason);
        report.subscriptions = self.cleanup_subscriptions(session_id);
        report.session_store = self.delete_session_store_row(session_id);
        report.registry = self.record_registry_closed(session_id, &registry_reason);
        report.finalize();
        if let Ok(mut registry) = self.session_registry.lock() {
            registry.record_cleanup_result(session_id, report.failure_count == 0);
        }
        if report.failure_count == 0 {
            tracing::info!(
                code = "MCP_SESSION_TEARDOWN_COMPLETED",
                session_id,
                reason = %report.reason,
                report = ?report,
                "readback=session_lifecycle after=all_owned_resources_reclaimed"
            );
        } else {
            tracing::error!(
                code = "MCP_SESSION_TEARDOWN_FAILED",
                session_id,
                reason = %report.reason,
                failure_count = report.failure_count,
                report = ?report,
                "session lifecycle teardown encountered cleanup failures"
            );
        }
        Ok(report)
    }

    pub(crate) async fn release_session_inputs_for_daemon_shutdown(
        &self,
        session_id: &str,
        reason: &str,
    ) -> SessionShutdownInputCleanupReport {
        let mut report = SessionShutdownInputCleanupReport {
            session_id: session_id.to_owned(),
            reason: reason.to_owned(),
            ..SessionShutdownInputCleanupReport::default()
        };
        if let Err(error) = validate_lifecycle_session_id(session_id) {
            report.failed = true;
            report.error_message = Some(error.message.to_string());
            return report;
        }
        report.browser_continuity = self.browser_continuity_for_daemon_shutdown(session_id);
        report.process_jobs =
            self.disarm_owned_process_jobs_for_daemon_shutdown(session_id, reason);
        report.input = self
            .cleanup_inputs_and_lease(session_id, "daemon_shutdown")
            .await;
        let seat_cleanup = self.cleanup_seats(session_id).await;
        report.lease_row_existed_before = report.input.lease_row_existed_before;
        report.lease_row_deleted = report.input.lease_row_deleted;
        report.lease_row_exists_after = report.input.lease_row_exists_after;
        if report.input.failed {
            report.failed = true;
        }
        if seat_cleanup.failed > 0 {
            report.failed = true;
            report.error_message.get_or_insert_with(|| {
                format!(
                    "{} isolated browser seat(s) failed to close during daemon shutdown",
                    seat_cleanup.failed
                )
            });
        }
        if report.process_jobs.failed > 0 {
            report.failed = true;
            if report.error_message.is_none() {
                report.error_message = Some(format!(
                    "{} owned process job(s) could not disarm kill-on-close before daemon shutdown",
                    report.process_jobs.failed
                ));
            }
        }
        if report.browser_continuity.failed {
            report.failed = true;
            if report.error_message.is_none() {
                report.error_message = report.browser_continuity.error_message.clone();
            }
        }
        tracing::info!(
            code = "MCP_SESSION_SHUTDOWN_INPUT_CLEANUP",
            session_id,
            reason,
            report = ?report,
            "readback=session_input_ownership edge=daemon_shutdown after_cleanup"
        );
        report
    }

    async fn cleanup_seats(&self, session_id: &str) -> SessionSeatCleanupReport {
        let owner = synapse_seats::OwnerId::new(session_id);
        let owned_before = self.seat_supervisor.list(&owner).await;
        let results = self.seat_supervisor.close_owner(&owner).await;
        let mut report = SessionSeatCleanupReport {
            source_of_truth: "SeatSupervisor registry + owned Chromium process/profile close readback",
            owned_before: owned_before.len(),
            ..SessionSeatCleanupReport::default()
        };
        for result in results {
            match result {
                Ok(_seat) => report.closed = report.closed.saturating_add(1),
                Err(error) => {
                    report.failed = report.failed.saturating_add(1);
                    report.failures.push(error.to_string());
                }
            }
        }
        report
    }

    fn disarm_owned_process_jobs_for_daemon_shutdown(
        &self,
        session_id: &str,
        reason: &str,
    ) -> SessionProcessRestartHandoffReport {
        let mut guard = match self.session_processes.lock() {
            Ok(guard) => guard,
            Err(_error) => {
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    session_id,
                    reason,
                    "session lifecycle could not lock process resources for daemon restart handoff"
                );
                return SessionProcessRestartHandoffReport {
                    failed: 1,
                    ..SessionProcessRestartHandoffReport::default()
                };
            }
        };
        let Some(resources) = guard.get_mut(session_id) else {
            return SessionProcessRestartHandoffReport::default();
        };
        let mut report = SessionProcessRestartHandoffReport {
            owned_before: resources.len(),
            ..SessionProcessRestartHandoffReport::default()
        };
        for resource in resources.values_mut() {
            let mut status = "no_job_handle".to_owned();
            let mut error_message = None;
            if let Some(process_job) = resource.process_job.as_mut() {
                report.job_handles_before = report.job_handles_before.saturating_add(1);
                match process_job.disarm_kill_on_close(
                    resource.tool,
                    resource.pid,
                    resource.resource_id.as_deref(),
                ) {
                    Ok(()) => {
                        report.kill_on_close_disarmed =
                            report.kill_on_close_disarmed.saturating_add(1);
                        status = "kill_on_close_disarmed".to_owned();
                    }
                    Err(error) => {
                        report.failed = report.failed.saturating_add(1);
                        status = "disarm_failed".to_owned();
                        error_message = Some(error.message.to_string());
                    }
                }
            }
            report.items.push(SessionProcessRestartHandoffItem {
                tool: resource.tool.to_owned(),
                pid: resource.pid,
                resource_id: resource.resource_id.clone(),
                launch_target: resource.launch_target.clone(),
                agent_cli: resource.agent_cli.clone(),
                registered_at_unix_ms: resource.registered_at_unix_ms,
                status,
                error_message,
            });
        }
        tracing::info!(
            code = "MCP_SESSION_PROCESS_RESTART_HANDOFF",
            session_id,
            reason,
            report = ?report,
            "readback=session_process_ledger edge=daemon_shutdown after=kill_on_close_disarmed"
        );
        report
    }

    pub(crate) async fn cleanup_expired_lease_inputs_once(&self) {
        // Lazy expiry: reading the lease is what moves a lapsed owner into the
        // expired-cleanup registry. Do it here so expiry is journaled
        // `lease_expired` and held inputs are released before the orphan sweep
        // can close the row as `orphan_sweep` (#95).
        let _lease_status_readback = synapse_action::lease::status();
        let pending = synapse_action::lease::expired_cleanup_snapshot();
        for expired in pending {
            let Some(session_id) = expired.owner_session_id.clone() else {
                continue;
            };
            let _transition = self.lease_transition_gate.lock().await;
            let before = self.action_handle.session_inputs_snapshot();
            let before_lease = synapse_action::lease::status();
            let result = self
                .action_handle
                .release_session_inputs_and_lease(&session_id)
                .await;
            let after = self.action_handle.session_inputs_snapshot();
            let after_lease = synapse_action::lease::status();
            match result {
                Ok(summary) => {
                    let reconciliation =
                        super::session_continuity::reconcile_persisted_session_lease_row(
                            &self.m3_state,
                            &session_id,
                            "lease_expired",
                        );
                    tracing::warn!(
                        code = "MCP_SESSION_LEASE_EXPIRED_INPUT_CLEANUP",
                        session_id,
                        released_keys = summary.input_summary.released_keys,
                        released_buttons = summary.input_summary.released_buttons,
                        neutralized_pads = summary.input_summary.neutralized_pads,
                        retained_shared_inputs = summary.input_summary.retained_shared_inputs,
                        input_lease_released = summary.lease_released,
                        expired_lease_cleanup_completed = summary.expired_lease_cleanup_completed,
                        expired = ?expired,
                        before = ?before,
                        after = ?after,
                        before_lease = ?before_lease,
                        after_lease = ?after_lease,
                        reconciliation = ?reconciliation,
                        "readback=session_input_ownership edge=input_lease_expired after_cleanup"
                    );
                }
                Err(error) => {
                    tracing::error!(
                        code = error.code(),
                        session_id,
                        detail = %error.detail(),
                        expired = ?expired,
                        before = ?before,
                        after = ?after,
                        before_lease = ?before_lease,
                        after_lease = ?after_lease,
                        "session lifecycle expired-lease cleanup failed while releasing owned inputs"
                    );
                }
            }
        }
    }

    /// Retry path for lease-release journaling (#93/#94/#95): closes persisted
    /// lease rows whose lease is gone, so a skipped or failed `LeaseReleased`
    /// write (expiry, emergency preempt, force-clear, crash) is repaired on the
    /// next sweep instead of leaving `held_open` elevated forever.
    pub(crate) async fn reconcile_orphaned_lease_rows_once(&self) {
        let _transition = self.lease_transition_gate.lock().await;
        match super::session_continuity::reconcile_orphaned_session_lease_rows(
            &self.m3_state,
            "orphan_sweep",
        ) {
            Ok(0) => {}
            Ok(closed) => tracing::warn!(
                code = "MCP_SESSION_LEASE_ORPHAN_SWEEP_CLOSED",
                closed,
                "readback=CF_SESSIONS+CF_AGENT_EVENTS after=orphaned_lease_rows_closed"
            ),
            Err(error) => tracing::error!(
                code = "MCP_SESSION_LEASE_ORPHAN_SWEEP_FAILED",
                error = %error,
                "orphaned session lease sweep could not scan CF_SESSIONS"
            ),
        }
    }

    /// #147: a crashed client's HTTP session stays "active" for the 24 h
    /// keep-alive, so its seats (a private Chromium + profile) are reclaimed
    /// after the shorter seat-owner MCP-idle limit. Closes ONLY the owner's
    /// seats; the session, leases, targets and clipboard are untouched so a
    /// connected-but-idle client keeps working. Returns seats closed.
    pub(crate) async fn reclaim_idle_seat_owners_once(&self) -> usize {
        let Some(limit_ms) = super::seat_owner_idle::seat_owner_idle_limit_from_env() else {
            return 0;
        };
        self.reclaim_idle_seat_owners(limit_ms).await
    }

    /// [`Self::reclaim_idle_seat_owners_once`] with an explicit MCP-idle limit
    /// instead of the env-configured one (test seam).
    pub(crate) async fn reclaim_idle_seat_owners(&self, limit_ms: u64) -> usize {
        let owners = match self.seat_supervisor.try_owner_ids() {
            Ok(owners) => owners,
            Err(error) => {
                tracing::warn!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    detail = %error,
                    "seat registry was busy during idle seat-owner scan; retrying next sweep"
                );
                return 0;
            }
        };
        if owners.is_empty() {
            return 0;
        }
        let registry_reads = match self.session_registry.lock() {
            Ok(registry) => registry.reads(unix_time_ms_now()),
            Err(_poisoned) => return 0,
        };
        // Only an SSE stream attached now counts; a subscription row alone
        // survives a client crash and must not keep the seat (#147).
        let streaming: BTreeSet<String> = self
            .sse_state
            .attached_owner_session_ids()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let activity: Vec<super::seat_owner_idle::SessionActivity<'_>> =
            registry_reads.iter().map(Into::into).collect();
        let idle = super::seat_owner_idle::idle_seat_owners(
            owners.iter().map(synapse_seats::OwnerId::as_str),
            &activity,
            &streaming,
            limit_ms,
        );
        let mut closed = 0_usize;
        for owner in idle {
            let report = self.cleanup_seats(&owner).await;
            if report.owned_before == 0 {
                continue;
            }
            closed = closed.saturating_add(report.closed);
            tracing::warn!(
                code = "MCP_SEAT_OWNER_IDLE_RECLAIM",
                session_id = %owner,
                limit_ms,
                reason = super::seat_owner_idle::SEAT_OWNER_IDLE_REASON,
                report = ?report,
                "readback=seat_registry edge=owner_mcp_idle after=seats_closed_session_kept"
            );
        }
        closed
    }

    pub(crate) fn stale_session_candidates(
        &self,
        active_sessions: &BTreeSet<String>,
    ) -> BTreeMap<String, &'static str> {
        // #1238: liveness for a spawned agent is an OS-process fact, never an
        // "MCP-request recency" fact. An agent doing native Read/Write/Bash
        // work makes zero Synapse calls for minutes, so the daemon legitimately
        // idle-evicts its MCP session (a readiness concern), but it must
        // never drop the kill-on-close job that owns the still-running child (a
        // *liveness* action). Read the registry once and partition spawned-agent
        // sessions by a real process probe: live ones are protected from
        // idle-driven teardown; exited ones remain reap candidates.
        let (registry_reads, failed_cleanup) = match self.session_registry.lock() {
            Ok(registry) => (
                registry.reads(unix_time_ms_now()),
                registry
                    .failed_cleanup_entries()
                    .into_iter()
                    .map(|entry| entry.session_id)
                    .collect::<BTreeSet<String>>(),
            ),
            Err(_poisoned) => {
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    "session registry lock poisoned during stale-candidate scan; spawned-agent liveness protection unavailable this pass"
                );
                (Vec::new(), BTreeSet::new())
            }
        };
        let (live_spawn_sessions, exited_spawn_sessions) =
            partition_spawned_sessions(&registry_reads, &|pid| m4::process_exists(pid));
        // Treat a live spawned-agent session as "active" everywhere below, so no
        // ownership ledger (inputs, lease, target, clipboard, process, CDP,
        // subscriptions) can flag it stale while its process is alive.
        let live: BTreeSet<String> = active_sessions
            .iter()
            .cloned()
            .chain(live_spawn_sessions)
            .collect();

        let mut candidates = BTreeMap::new();
        if let Ok(snapshot) = self.action_handle.session_inputs_snapshot() {
            for session in snapshot.sessions {
                add_if_stale(
                    &mut candidates,
                    &live,
                    &session.session_id,
                    HTTP_STALE_REASON,
                );
            }
        }
        let lease_status = synapse_action::lease::status();
        if let Some(owner) = lease_status.owner_session_id.as_ref()
            && owner != synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID
        {
            add_if_stale(&mut candidates, &live, owner, HTTP_STALE_REASON);
        }
        if let Ok(targets) = self.session_targets.lock() {
            for session_id in targets.keys() {
                add_if_stale(&mut candidates, &live, session_id, HTTP_STALE_REASON);
            }
        }
        match super::session_continuity::persisted_session_target_session_ids(&self.m3_state) {
            Ok(session_ids) => {
                for session_id in session_ids {
                    if self.should_defer_restart_browser_stale_cleanup(&session_id) {
                        continue;
                    }
                    add_if_stale(&mut candidates, &live, &session_id, HTTP_STALE_REASON);
                }
            }
            Err(detail) => {
                tracing::error!(
                    code = error_codes::STORAGE_CORRUPTED,
                    detail = %detail,
                    "session lifecycle could not scan persisted session target rows for stale candidates"
                );
            }
        }
        if let Ok(state) = self.m3_state.lock() {
            for session_id in state.mcp_audit_sessions.keys() {
                add_if_stale(&mut candidates, &live, session_id, HTTP_STALE_REASON);
            }
        }
        if let Ok(owners) = self.cdp_target_owners.lock() {
            for owner in owners.values() {
                add_if_stale(&mut candidates, &live, &owner.session_id, HTTP_STALE_REASON);
            }
        }
        match super::session_continuity::persisted_cdp_target_owner_session_ids(&self.m3_state) {
            Ok(session_ids) => {
                for session_id in session_ids {
                    if self.should_defer_restart_browser_stale_cleanup(&session_id) {
                        continue;
                    }
                    add_if_stale(&mut candidates, &live, &session_id, HTTP_STALE_REASON);
                }
            }
            Err(detail) => {
                tracing::error!(
                    code = error_codes::STORAGE_CORRUPTED,
                    detail = %detail,
                    "session lifecycle could not scan persisted CDP target owner rows for stale candidates"
                );
            }
        }
        if let Ok(claims) = self.target_claims.lock() {
            for read in claims.reads(unix_time_ms_now()) {
                add_if_stale(
                    &mut candidates,
                    &live,
                    &read.owner_session_id,
                    HTTP_STALE_REASON,
                );
            }
        }
        if let Ok(clipboards) = self.session_clipboards.lock() {
            for session_id in clipboards.keys() {
                add_if_stale(&mut candidates, &live, session_id, HTTP_STALE_REASON);
            }
        }
        if let Ok(processes) = self.session_processes.lock() {
            for session_id in processes.keys() {
                add_if_stale(&mut candidates, &live, session_id, HTTP_STALE_REASON);
            }
        }
        if let Ok(session_ids) = self.sse_state.subscription_owner_session_ids() {
            for session_id in session_ids {
                add_if_stale(&mut candidates, &live, &session_id, HTTP_STALE_REASON);
            }
        }
        match self.seat_supervisor.try_owner_ids() {
            Ok(owners) => {
                for owner in owners {
                    add_if_stale(&mut candidates, &live, owner.as_str(), HTTP_STALE_REASON);
                }
            }
            Err(error) => tracing::warn!(
                code = error_codes::TOOL_INTERNAL_ERROR,
                detail = %error,
                "seat registry was busy during stale-session candidate scan; retrying next sweep"
            ),
        }
        // Non-spawn (operator/human) sessions still reap on MCP-idle staleness
        // for input/lease/target cleanup. Spawned-agent sessions are
        // intentionally excluded here; their liveness is governed solely by the
        // process probe (protected above when alive, reaped below when gone).
        for read in &registry_reads {
            if read.lifecycle == "stale" && read.spawned_agent.is_none() {
                add_if_stale(&mut candidates, &live, &read.session_id, HTTP_STALE_REASON);
            }
        }
        // #75: an OS-dead spawned owner is reaped even while its HTTP session
        // is still in the manager map (abrupt death skips DELETE, leaving the
        // session "active" for the 24 h keep-alive). Only failed-cleanup
        // ownership stays protected; it is retried by its own path.
        for read in exited_spawn_sessions {
            if failed_cleanup.contains(&read.session_id) {
                continue;
            }
            let reason = spawned_agent_exit_reason(read);
            if candidates.insert(read.session_id.clone(), reason) != Some(reason) {
                tracing::warn!(
                    code = "MCP_SESSION_SPAWNED_AGENT_PROCESS_EXITED",
                    session_id = %read.session_id,
                    reason,
                    spawned_agent = ?read.spawned_agent,
                    active_session_count = active_sessions.len(),
                    "readback=session_registry edge=spawned_primary_process_gone before=session_teardown_candidate"
                );
            }
        }
        // #213: never re-select a session whose teardown is still running.
        candidates.retain(|session_id, _reason| !self.session_teardown_in_progress(session_id));
        candidates
    }

    pub(crate) fn live_spawned_session_ids_for_shutdown(&self) -> BTreeSet<String> {
        let registry_reads = match self.session_registry.lock() {
            Ok(registry) => registry.reads(unix_time_ms_now()),
            Err(_poisoned) => {
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    "session registry lock poisoned during daemon-shutdown spawned-agent liveness scan; restart handoff cannot include idle-evicted spawned sessions this pass"
                );
                Vec::new()
            }
        };
        let live_spawn_sessions =
            live_spawned_session_ids(&registry_reads, &|pid| m4::process_exists(pid));
        if !live_spawn_sessions.is_empty() {
            tracing::info!(
                code = "MCP_SESSION_LIVE_SPAWNED_SHUTDOWN_HANDOFF_CANDIDATES",
                live_spawn_session_count = live_spawn_sessions.len(),
                session_ids = ?live_spawn_sessions,
                "readback=session_registry edge=daemon_shutdown before=live_spawned_process_handoff"
            );
        }
        live_spawn_sessions
    }

    fn browser_continuity_for_daemon_shutdown(
        &self,
        session_id: &str,
    ) -> SessionShutdownBrowserContinuityReport {
        let mut errors = Vec::new();
        let memory_target = match self.session_targets.lock() {
            Ok(targets) => targets
                .get(session_id)
                .map(session_target_shutdown_readback),
            Err(_error) => {
                errors.push("session target registry lock poisoned".to_owned());
                None
            }
        };
        let persisted_target =
            match super::session_continuity::read_persisted_session_target_for_session(
                &self.m3_state,
                session_id,
            ) {
                Ok(target) => target.as_ref().map(session_target_shutdown_readback),
                Err(error) => {
                    errors.push(format!("read persisted session target failed: {error}"));
                    None
                }
            };
        let memory_owners = match self.memory_cdp_target_owner_readbacks_for_session(session_id) {
            Ok(owners) => owners,
            Err(error) => {
                errors.push(error);
                Vec::new()
            }
        };
        let persisted_owners =
            match persisted_cdp_target_owner_readbacks_for_session(&self.m3_state, session_id) {
                Ok(owners) => owners,
                Err(error) => {
                    errors.push(format!(
                        "read persisted CDP target owner rows failed: {error}"
                    ));
                    Vec::new()
                }
            };
        let memory_cdp_owner_count_before = memory_owners.len();
        let persisted_cdp_owner_count_before = persisted_owners.len();
        let mut cdp_target_owners = Vec::with_capacity(
            memory_cdp_owner_count_before.saturating_add(persisted_cdp_owner_count_before),
        );
        let mut seen = BTreeSet::new();
        for owner in memory_owners.into_iter().chain(persisted_owners) {
            if seen.insert(owner.owner_key.clone()) {
                cdp_target_owners.push(owner);
            }
        }
        let has_browser_continuity = matches!(
            memory_target.as_ref().or(persisted_target.as_ref()),
            Some(SessionTargetShutdownReadback::Cdp { .. })
        ) || !cdp_target_owners.is_empty();
        SessionShutdownBrowserContinuityReport {
            source_of_truth: "in-memory session target/CDP owner registries + CF_SESSIONS persisted target/CDP owner rows".to_owned(),
            recovery_action: if has_browser_continuity {
                "daemon restart preserves browser target rows; create a new MCP session, inspect session_list/session_status(include_closed), then rebind/adopt the same already-open Chrome tab before retrying or proving completion".to_owned()
            } else {
                "no browser target or CDP owner row was present for this session at daemon shutdown".to_owned()
            },
            restart_grace_remaining_ms: self.restart_browser_continuity_grace_remaining_ms(),
            memory_target_before: memory_target,
            persisted_target_before: persisted_target,
            memory_cdp_owner_count_before,
            persisted_cdp_owner_count_before,
            cdp_target_owners,
            tab_close_attempted: false,
            tab_close_succeeded: false,
            failed: !errors.is_empty(),
            error_message: (!errors.is_empty()).then(|| errors.join("; ")),
        }
    }

    fn memory_cdp_target_owner_readbacks_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionCdpTargetOwnerReadback>, String> {
        let guard = self
            .cdp_target_owners
            .lock()
            .map_err(|_error| "CDP target ownership registry lock poisoned".to_owned())?;
        let mut owners = guard
            .iter()
            .filter_map(|(owner_key, owner)| {
                (owner.session_id == session_id)
                    .then(|| session_cdp_target_owner_readback(owner_key.clone(), owner.clone()))
            })
            .collect::<Vec<_>>();
        owners.sort_by(|left, right| left.owner_key.cmp(&right.owner_key));
        Ok(owners)
    }

    fn restart_browser_continuity_grace_remaining_ms(&self) -> u64 {
        let elapsed = self.started_at.elapsed();
        DAEMON_RESTART_BROWSER_CONTINUITY_GRACE
            .checked_sub(elapsed)
            .map(duration_millis_u64)
            .unwrap_or(0)
    }

    fn daemon_restart_browser_continuity_grace_active(&self) -> bool {
        self.restart_browser_continuity_grace_remaining_ms() > 0
    }

    fn should_defer_restart_browser_stale_cleanup(&self, session_id: &str) -> bool {
        if !self.daemon_restart_browser_continuity_grace_active() {
            return false;
        }
        match self.session_has_persisted_browser_continuity(session_id) {
            Ok(true) => true,
            Ok(false) => false,
            Err(error) => {
                tracing::error!(
                    code = error_codes::STORAGE_CORRUPTED,
                    session_id,
                    detail = %error,
                    "restart browser continuity deferral could not read persisted session rows"
                );
                false
            }
        }
    }

    fn session_has_persisted_browser_continuity(&self, session_id: &str) -> Result<bool, String> {
        let target = super::session_continuity::read_persisted_session_target_for_session(
            &self.m3_state,
            session_id,
        )?;
        if matches!(target, Some(SessionTarget::Cdp { .. })) {
            return Ok(true);
        }
        let owners = super::session_continuity::read_persisted_cdp_target_owners_for_session(
            &self.m3_state,
            session_id,
        )?;
        Ok(!owners.is_empty())
    }

    fn mark_terminated_session(&self, report: &mut SessionTeardownReport) {
        match self.terminated_sessions.lock() {
            Ok(mut terminated) => {
                report.already_terminated = terminated.contains(&report.session_id);
                let evicted_before = terminated.stats().evicted_total;
                report.marked_terminated = terminated.insert(report.session_id.clone());
                let stats = terminated.stats();
                if stats.evicted_total > evicted_before {
                    tracing::info!(
                        code = "SESSION_TERMINATED_REGISTRY_EVICTED",
                        occupancy = stats.len,
                        capacity = stats.capacity,
                        evicted_total = stats.evicted_total,
                        "terminated-session registry full; oldest ids forgotten"
                    );
                }
            }
            Err(_error) => {
                report.termination_marker_failed = true;
                report.termination_marker_error_message =
                    Some("terminated-session registry lock poisoned".to_owned());
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    session_id = %report.session_id,
                    "session lifecycle could not lock terminated-session registry"
                );
            }
        }
    }

    async fn cleanup_inputs_and_lease(
        &self,
        session_id: &str,
        reason_code: &str,
    ) -> SessionInputCleanupReport {
        // Let an in-flight act_foreground escalation of this session unwind
        // first (#53); taken before the transition gate its unwind needs.
        super::act_foreground_slot::await_session_escalation(session_id).await;
        let _transition = self.lease_transition_gate.lock().await;
        let before_snapshot = self.action_handle.session_inputs_snapshot();
        let before_lease = synapse_action::lease::status();
        let mut report = SessionInputCleanupReport {
            snapshot_read_before: before_snapshot.is_ok(),
            owned_inputs_before: before_snapshot
                .as_ref()
                .map_or(0, |snapshot| owned_input_count(snapshot, session_id)),
            lease_held_before: before_lease.held,
            lease_owner_before: before_lease.owner_session_id.clone(),
            ..SessionInputCleanupReport::default()
        };
        match self
            .action_handle
            .release_session_inputs_and_lease(session_id)
            .await
        {
            Ok(summary) => {
                report.released_keys = summary.input_summary.released_keys;
                report.released_buttons = summary.input_summary.released_buttons;
                report.neutralized_pads = summary.input_summary.neutralized_pads;
                report.retained_shared_inputs = summary.input_summary.retained_shared_inputs;
                report.lease_released = summary.lease_released;
                report.expired_lease_cleanup_completed = summary.expired_lease_cleanup_completed;
                match super::session_continuity::reconcile_persisted_session_lease_row(
                    &self.m3_state,
                    session_id,
                    reason_code,
                ) {
                    Ok(readback) => {
                        report.lease_row_existed_before = readback.row_existed_before;
                        report.lease_row_deleted = readback.row_deleted;
                        report.lease_row_exists_after = readback.row_exists_after;
                        report.lease_release_event_written = readback.release_event_written;
                    }
                    Err(error) => {
                        report.failed = true;
                        report.error_code = Some(error_codes::STORAGE_CORRUPTED.to_owned());
                        report.error_message = Some(format!(
                            "input cleanup completed but persisted lease reconciliation failed: {error}"
                        ));
                    }
                }
            }
            Err(error) => {
                report.failed = true;
                report.error_code = Some(error.code().to_owned());
                report.error_message = Some(error.detail().to_owned());
            }
        }
        let after_snapshot = self.action_handle.session_inputs_snapshot();
        let after_lease = synapse_action::lease::status();
        report.snapshot_read_after = after_snapshot.is_ok();
        report.owned_inputs_after = after_snapshot
            .as_ref()
            .map_or(0, |snapshot| owned_input_count(snapshot, session_id));
        report.lease_held_after = after_lease.held;
        report.lease_owner_after = after_lease.owner_session_id;
        if after_snapshot.is_err() {
            report.failed = true;
            if report.error_message.is_none() {
                report.error_code = Some(error_codes::TOOL_INTERNAL_ERROR.to_owned());
                report.error_message = Some("input ownership after-read failed".to_owned());
            }
        }
        report
    }

    fn cleanup_target(&self, session_id: &str) -> SessionTargetCleanupReport {
        // #299: the session's agents lose their targets with the session.
        let agent_targets_cleared = super::agent_scope::forget_session(session_id);
        if agent_targets_cleared > 0 {
            tracing::info!(
                code = "MCP_SESSION_AGENT_TARGETS_CLEARED",
                session_id,
                agent_targets_cleared,
                "readback=agent_target_registry outcome=session_agents_cleared"
            );
        }
        match self.session_targets.lock() {
            Ok(mut targets) => {
                let before = targets.len();
                let target_cleared = targets.remove(session_id).is_some();
                let after = targets.len();
                SessionTargetCleanupReport {
                    target_cleared,
                    target_sessions_before: before,
                    target_sessions_after: after,
                    failed: false,
                    error_message: None,
                }
            }
            Err(_error) => SessionTargetCleanupReport {
                failed: true,
                error_message: Some("session target registry lock poisoned".to_owned()),
                ..SessionTargetCleanupReport::default()
            },
        }
    }

    fn cleanup_target_claims(&self, session_id: &str) -> TargetClaimCleanupReport {
        target_claims::cleanup_claims_for_session(&self.target_claims, session_id)
    }

    fn cleanup_continuity(&self, session_id: &str) -> SessionContinuityCleanupReport {
        match super::session_continuity::delete_persisted_session_target_row(
            &self.m3_state,
            session_id,
        ) {
            Ok(readback) => SessionContinuityCleanupReport {
                target_row_existed_before: readback.row_existed_before,
                target_row_deleted: readback.row_deleted,
                target_row_exists_after: readback.row_exists_after,
                lease_row_existed_before: false,
                lease_row_deleted: false,
                lease_row_exists_after: false,
                failed: false,
                error_message: None,
            },
            Err(error) => {
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    session_id,
                    detail = %error,
                    "session lifecycle failed to delete persisted target continuity row"
                );
                SessionContinuityCleanupReport {
                    failed: true,
                    error_message: Some(error),
                    ..SessionContinuityCleanupReport::default()
                }
            }
        }
    }

    fn cleanup_audit_session(&self, session_id: &str) -> SessionAuditSessionCleanupReport {
        match self.m3_state.lock() {
            Ok(mut state) => {
                let before = state.mcp_audit_sessions.len();
                let removed = state.mcp_audit_sessions.remove(session_id).is_some();
                let after = state.mcp_audit_sessions.len();
                tracing::info!(
                    code = "MCP_SESSION_AUDIT_CACHE_CLEANUP",
                    session_id,
                    before,
                    after,
                    removed,
                    "readback=m3_state.mcp_audit_sessions after=session_cache_removed"
                );
                SessionAuditSessionCleanupReport {
                    cache_sessions_before: before,
                    cache_sessions_after: after,
                    removed,
                    failed: false,
                    error_message: None,
                }
            }
            Err(_error) => SessionAuditSessionCleanupReport {
                failed: true,
                error_message: Some("M3 service state lock poisoned".to_owned()),
                ..SessionAuditSessionCleanupReport::default()
            },
        }
    }

    fn cleanup_clipboard(&self, session_id: &str) -> SessionClipboardCleanupReport {
        match self.session_clipboards.lock() {
            Ok(mut clipboards) => {
                let buffer_count_before = clipboards.len();
                let buffer_existed_before = clipboards.contains_key(session_id);
                let removed = clipboards.remove(session_id).is_some();
                let buffer_exists_after = clipboards.contains_key(session_id);
                let buffer_count_after = clipboards.len();
                tracing::info!(
                    code = "MCP_SESSION_CLIPBOARD_CLEANUP",
                    session_id,
                    buffer_count_before,
                    buffer_count_after,
                    buffer_existed_before,
                    removed,
                    buffer_exists_after,
                    "readback=session_clipboard_buffer after=session_buffer_removed"
                );
                SessionClipboardCleanupReport {
                    buffer_existed_before,
                    buffer_count_before,
                    removed,
                    buffer_exists_after,
                    buffer_count_after,
                    failed: false,
                    error_message: None,
                }
            }
            Err(_error) => SessionClipboardCleanupReport {
                failed: true,
                error_message: Some("session clipboard registry lock poisoned".to_owned()),
                ..SessionClipboardCleanupReport::default()
            },
        }
    }

    /// #212: process-tree cleanup polls the OS with blocking sleeps (up to
    /// ~35 s per spawned-agent resource). Run it on the blocking pool so a
    /// teardown never parks a tokio worker thread.
    async fn cleanup_owned_processes_off_worker(
        &self,
        session_id: &str,
        options: SessionTeardownOptions,
        teardown_permit: Arc<teardown_gate::SessionTeardownPermit<'static>>,
    ) -> SessionProcessCleanupReport {
        let state = self.clone();
        let owned_session_id = session_id.to_owned();
        match tokio::task::spawn_blocking(move || {
            // #213: the permit lives until this blocking work ends.
            let _teardown_permit = teardown_permit;
            state.cleanup_owned_processes(&owned_session_id, options)
        })
        .await
        {
            Ok(report) => report,
            Err(error) => {
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    session_id,
                    detail = %error,
                    "session process cleanup task did not complete"
                );
                SessionProcessCleanupReport {
                    failed: 1,
                    ..SessionProcessCleanupReport::default()
                }
            }
        }
    }

    fn cleanup_owned_processes(
        &self,
        session_id: &str,
        options: SessionTeardownOptions,
    ) -> SessionProcessCleanupReport {
        self.cleanup_owned_processes_with(session_id, options, &m4::terminate_owned_process_tree)
    }

    fn cleanup_owned_processes_with(
        &self,
        session_id: &str,
        options: SessionTeardownOptions,
        terminate: &dyn Fn(u32) -> m4::OwnedProcessTerminationReadback,
    ) -> SessionProcessCleanupReport {
        let resources = match self.session_processes.lock() {
            Ok(mut processes) => processes.remove(session_id).unwrap_or_default(),
            Err(_error) => {
                return SessionProcessCleanupReport {
                    failed: 1,
                    ..SessionProcessCleanupReport::default()
                };
            }
        };
        let mut report = SessionProcessCleanupReport {
            owned_before: resources.len(),
            ..SessionProcessCleanupReport::default()
        };
        let mut retained = BTreeMap::new();
        for mut resource in resources.into_values() {
            let process_ids = resource_process_ids(&resource);
            let live_before = m4::owned_live_process_ids(&process_ids);
            let job_handle_dropped = resource.process_job.is_some();
            let agent_spawn_cleanup = AgentSpawnCleanupRead::for_resource(&resource);
            let mut natural_exit_wait_ms = 0;
            let mut completion_status_before_cleanup = agent_spawn_cleanup
                .as_ref()
                .and_then(|cleanup| cleanup.completion_status());
            let mut completion_artifact_cleanup_status = None;
            let mut completion_artifact_cleanup_error = None;
            let mut remaining = live_before.clone();
            let mut remaining_after_natural_wait = live_before.clone();
            let desktop_name = resource
                .desktop_lease
                .as_ref()
                .map(|desktop| desktop.name().to_owned());

            if agent_spawn_cleanup.is_some()
                && !remaining.is_empty()
                && !options.agent_spawn_completion_grace.is_zero()
            {
                let (after_natural_wait, waited_ms) = m4::wait_for_owned_process_tree_exit(
                    &process_ids,
                    options.agent_spawn_completion_grace,
                );
                natural_exit_wait_ms = waited_ms;
                remaining_after_natural_wait = after_natural_wait;
                completion_status_before_cleanup = agent_spawn_cleanup
                    .as_ref()
                    .and_then(|cleanup| cleanup.completion_status());
            }

            if job_handle_dropped {
                report.job_close_attempted = report.job_close_attempted.saturating_add(1);
            }
            drop(resource.process_job.take());
            let (after_job_drop, _waited_ms) =
                m4::wait_for_owned_process_tree_exit(&process_ids, PROCESS_JOB_CLOSE_WAIT);
            remaining = after_job_drop;
            let job_close_terminated_processes =
                !remaining_after_natural_wait.is_empty() && remaining.is_empty();
            let mut force_termination_status = None;
            if !remaining.is_empty() {
                let remaining_before_force = remaining.clone();
                report.force_termination_attempted =
                    report.force_termination_attempted.saturating_add(1);
                let forced = terminate(resource.pid);
                force_termination_status = Some(forced.status);
                remaining = forced.remaining_process_ids;
                // Survivors retained from an earlier failed pass may no longer
                // hang under the root pid; terminate each by identity.
                let orphans = remaining_before_force
                    .iter()
                    .copied()
                    .filter(|pid| *pid != resource.pid)
                    .filter(|pid| resource.retained_processes.contains_key(pid))
                    .filter(|pid| !remaining.contains(pid))
                    .collect::<Vec<_>>();
                for orphan in orphans {
                    remaining.extend(terminate(orphan).remaining_process_ids);
                }
                remaining.sort_unstable();
                remaining.dedup();
            }
            // #75 debug-only fault hook: report a survivor so reviewers can
            // verify ownership retention and the verified retry live.
            #[cfg(debug_assertions)]
            if let Some(pid) = cleanup_fault_hook::forced_failure_pid(&process_ids) {
                remaining.push(pid);
                remaining.sort_unstable();
                remaining.dedup();
            }
            if let Some(cleanup) = &agent_spawn_cleanup {
                if !cleanup.has_terminal_completion_status() {
                    let status = if force_termination_status.is_some() {
                        "session_cleanup_forced"
                    } else if job_close_terminated_processes {
                        "session_cleanup_job_closed"
                    } else {
                        "session_cleanup_no_completion"
                    };
                    match cleanup.write_terminal_artifact(
                        status,
                        &resource,
                        &process_ids,
                        &live_before,
                        &remaining,
                        natural_exit_wait_ms,
                        job_close_terminated_processes,
                        force_termination_status.as_deref(),
                    ) {
                        Ok(write_status) => {
                            completion_artifact_cleanup_status = Some(write_status);
                        }
                        Err(error) => {
                            completion_artifact_cleanup_error = Some(error);
                        }
                    }
                }
                // #900: the spawn's processes are gone (or force-terminated)
                // by here, so its stdout stream is final — flush the
                // transcript tail and mark the source complete.
                match session_store_db(&self.m3_state) {
                    Ok(db) => super::agent_transcripts::finalize_spawn_transcripts(
                        &db,
                        &cleanup.spawn_id,
                        &cleanup.log_dir,
                    ),
                    Err(error) => tracing::error!(
                        code = "TRANSCRIPT_TEARDOWN_FLUSH_FAILED",
                        spawn_id = %cleanup.spawn_id,
                        detail = %error,
                        "teardown transcript flush could not access storage"
                    ),
                }
            }
            let desktop_close = resource
                .desktop_lease
                .take()
                .map(m4::LaunchDesktopLease::close);
            let desktop_cleanup_succeeded =
                desktop_close.as_ref().is_none_or(|close| close.succeeded);
            let desktop_close_name = desktop_close.as_ref().map(|close| close.name.clone());
            let desktop_window_process_ids_before = desktop_close
                .as_ref()
                .map_or_else(Vec::new, |close| close.window_process_ids_before.clone());
            let desktop_window_termination_attempted = desktop_close
                .as_ref()
                .is_some_and(|close| close.window_termination_attempted);
            let desktop_window_termination_status = desktop_close
                .as_ref()
                .and_then(|close| close.window_termination_status.clone());
            let desktop_window_process_ids_after = desktop_close
                .as_ref()
                .map_or_else(Vec::new, |close| close.window_process_ids_after.clone());
            if !desktop_window_process_ids_after.is_empty() {
                remaining.extend(desktop_window_process_ids_after.iter().copied());
                remaining.sort_unstable();
                remaining.dedup();
            }
            if remaining.is_empty() && desktop_cleanup_succeeded {
                report.terminated = report.terminated.saturating_add(1);
            } else {
                report.failed = report.failed.saturating_add(1);
            }
            let retain_for_retry = !remaining.is_empty();
            report.items.push(SessionProcessCleanupItem {
                tool: resource.tool.to_owned(),
                pid: resource.pid,
                resource_id: resource.resource_id.clone(),
                launch_target: resource.launch_target.clone(),
                agent_cli: resource.agent_cli.clone(),
                registered_at_unix_ms: resource.registered_at_unix_ms,
                process_ids_before: process_ids,
                live_process_ids_before: live_before,
                job_handle_dropped,
                natural_exit_wait_ms,
                force_termination_status,
                completion_status_path: agent_spawn_cleanup
                    .as_ref()
                    .map(|cleanup| cleanup.completion_status_path.display().to_string()),
                completion_status_before_cleanup,
                completion_artifact_cleanup_status,
                completion_artifact_cleanup_error,
                desktop_name: desktop_close_name.or(desktop_name),
                desktop_close_attempted: desktop_close
                    .as_ref()
                    .is_some_and(|close| close.attempted),
                desktop_close_succeeded: desktop_close.as_ref().map(|close| close.succeeded),
                desktop_close_error: desktop_close.and_then(|close| close.error_message),
                desktop_window_process_ids_before,
                desktop_window_termination_attempted,
                desktop_window_termination_status,
                desktop_window_process_ids_after,
                remaining_process_ids_after: remaining.clone(),
            });
            if retain_for_retry {
                // #75: keep ownership of everything that failed teardown; the
                // failure flag may only clear once a retry verifies exit.
                let start_times = m4::process_start_times(&remaining);
                resource.retained_processes = remaining
                    .iter()
                    .map(|pid| (*pid, start_times.get(pid).copied().unwrap_or(0)))
                    .collect();
                retained.insert(resource.pid, resource);
            }
        }
        if !retained.is_empty() {
            match self.session_processes.lock() {
                Ok(mut processes) => processes
                    .entry(session_id.to_owned())
                    .or_default()
                    .extend(retained),
                Err(_error) => report.failed = report.failed.saturating_add(1),
            }
        }
        report
    }

    fn cleanup_subscriptions(&self, session_id: &str) -> SessionSubscriptionCleanupReport {
        match self.sse_state.subscription_ids_for_session(session_id) {
            Ok(ids_before) => match self.sse_state.cancel_session_subscriptions(session_id) {
                Ok(cancelled) => SessionSubscriptionCleanupReport {
                    owned_before: ids_before.len(),
                    cancelled: cancelled.len(),
                    subscription_ids: cancelled,
                    failed: false,
                    error_code: None,
                    error_message: None,
                },
                Err(error) => SessionSubscriptionCleanupReport {
                    owned_before: ids_before.len(),
                    failed: true,
                    error_code: Some(error.code().to_owned()),
                    error_message: Some(error.message(session_id)),
                    ..SessionSubscriptionCleanupReport::default()
                },
            },
            Err(error) => SessionSubscriptionCleanupReport {
                failed: true,
                error_code: Some(error.code().to_owned()),
                error_message: Some(error.message(session_id)),
                ..SessionSubscriptionCleanupReport::default()
            },
        }
    }

    fn delete_session_store_row(&self, session_id: &str) -> SessionStoreCleanupReport {
        let _row_gate = lock_session_store_rows();
        let key = mcp_session_store_key(session_id);
        let key_string = String::from_utf8_lossy(&key).into_owned();
        let db = match session_store_db(&self.m3_state) {
            Ok(db) => db,
            Err(error) => {
                return SessionStoreCleanupReport {
                    key: key_string,
                    failed: true,
                    error_message: Some(error),
                    ..SessionStoreCleanupReport::default()
                };
            }
        };
        let existed_before = match session_store_row_exists(&db, &key) {
            Ok(exists) => exists,
            Err(error) => {
                return SessionStoreCleanupReport {
                    key: key_string,
                    failed: true,
                    error_message: Some(error),
                    ..SessionStoreCleanupReport::default()
                };
            }
        };
        if existed_before && let Err(error) = db.delete_batch(cf::CF_KV, [key.clone()]) {
            return SessionStoreCleanupReport {
                key: key_string,
                existed_before,
                failed: true,
                error_message: Some(error.to_string()),
                ..SessionStoreCleanupReport::default()
            };
        }
        let exists_after = match session_store_row_exists(&db, &key) {
            Ok(exists) => exists,
            Err(error) => {
                return SessionStoreCleanupReport {
                    key: key_string,
                    existed_before,
                    deleted: existed_before,
                    failed: true,
                    error_message: Some(error),
                    ..SessionStoreCleanupReport::default()
                };
            }
        };
        SessionStoreCleanupReport {
            key: key_string,
            existed_before,
            deleted: existed_before && !exists_after,
            exists_after,
            failed: exists_after,
            error_message: exists_after
                .then(|| "session store row still exists after delete".to_owned()),
        }
    }

    fn record_registry_closed(
        &self,
        session_id: &str,
        reason: &str,
    ) -> SessionRegistryCleanupReport {
        let transitioned = match self.session_registry.lock() {
            Ok(mut registry) => {
                let retry_journal = registry.closure_journal_failed(session_id);
                registry.record_closed_with_reason(session_id, unix_time_ms_now(), Some(reason))
                    || retry_journal
            }
            Err(_error) => {
                return SessionRegistryCleanupReport {
                    closed_recorded: false,
                    reason_code: reason.to_owned(),
                    journal_event_written: false,
                    failed: true,
                    error_message: Some("session registry lock poisoned".to_owned()),
                };
            }
        };
        let mut report = SessionRegistryCleanupReport {
            closed_recorded: true,
            reason_code: reason.to_owned(),
            journal_event_written: false,
            failed: false,
            error_message: None,
        };
        if transitioned {
            // Terminal lifecycle fact: journal it durably (#897). The agent's
            // task outcome is unknown to teardown, hence `indeterminate`.
            match self.journal_session_exited_event(session_id, reason) {
                Ok(()) => {
                    report.journal_event_written = true;
                    if let Ok(mut registry) = self.session_registry.lock() {
                        registry.record_closure_journal_result(session_id, true);
                    }
                }
                Err(error) => {
                    if let Ok(mut registry) = self.session_registry.lock() {
                        registry.record_closure_journal_result(session_id, false);
                    }
                    report.failed = true;
                    report.error_message = Some(error);
                }
            }
        }
        report
    }

    fn journal_session_exited_event(&self, session_id: &str, reason: &str) -> Result<(), String> {
        let db = session_store_db(&self.m3_state)?;
        let mut record = synapse_core::AgentEventRecord::new(
            super::agent_events::unix_time_ns_now(),
            synapse_core::AgentEventKind::Exited,
        );
        record.session_id = Some(session_id.to_owned());
        record.reason_code = Some(reason.to_owned());
        record.end_state = Some(synapse_core::AgentEndState::Indeterminate);
        record.attributes.conversation_id = Some(session_id.to_owned());
        super::agent_events::record_agent_event_durable(&db, &record)
            .map(|_readback| ())
            .map_err(|error| format!("journal session exited event: {error}"))
    }
}

/// Process tree of the resource root plus survivors retained by an earlier
/// failed teardown, keeping only retained pids that still match the recorded
/// start time (a recycled pid must never be treated as owned).
fn resource_process_ids(resource: &SessionProcessResource) -> Vec<u32> {
    let mut ids = m4::owned_process_tree_ids(resource.pid);
    let current = m4::process_start_times(
        &resource
            .retained_processes
            .keys()
            .copied()
            .collect::<Vec<_>>(),
    );
    ids.extend(
        resource
            .retained_processes
            .iter()
            .filter_map(|(pid, start)| {
                (current
                    .get(pid)
                    .is_some_and(|now| *start == 0 || now == start))
                .then_some(*pid)
            }),
    );
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// #212: shell-job cleanup waits on process trees with blocking sleeps; keep it
/// off the async workers.
async fn cleanup_shell_jobs_off_worker(
    session_id: &str,
    reason: &str,
    teardown_permit: Arc<teardown_gate::SessionTeardownPermit<'static>>,
) -> SessionShellCleanupReport {
    let owned_session_id = session_id.to_owned();
    let owned_reason = reason.to_owned();
    match crate::m4::spawn_shell_job_blocking(move || {
        // #213: the permit lives until this blocking work ends.
        let _teardown_permit = teardown_permit;
        cleanup_shell_jobs(&owned_session_id, &owned_reason)
    })
    .await
    {
        Ok(report) => report,
        Err(error) => {
            tracing::error!(
                code = error_codes::TOOL_INTERNAL_ERROR,
                session_id,
                detail = %error,
                "session shell-job cleanup task did not complete"
            );
            SessionShellCleanupReport {
                failed: 1,
                error_message: Some(format!("shell-job cleanup task did not complete: {error}")),
                ..SessionShellCleanupReport::default()
            }
        }
    }
}

fn cleanup_shell_jobs(session_id: &str, reason: &str) -> SessionShellCleanupReport {
    match m4::cleanup_shell_jobs_for_session(session_id, reason) {
        Ok(readback) => SessionShellCleanupReport {
            job_root: readback.job_root,
            status_files_read: readback.status_files_read,
            skipped_invalid_job_dirs: readback.skipped_invalid_job_dirs,
            skipped_unreadable_status_files: readback.skipped_unreadable_status_files,
            skipped_foreign_jobs: readback.skipped_foreign_jobs,
            live_jobs_before: readback.live_jobs_before,
            retained_live_jobs: readback.retained_live_jobs,
            termination_attempted: readback.termination_attempted,
            termination_succeeded: readback.termination_succeeded,
            failed: readback.failed,
            job_ids: readback.job_ids,
            remaining_process_ids: readback.remaining_process_ids,
            error_code: None,
            error_message: None,
        },
        Err(error) => SessionShellCleanupReport {
            failed: 1,
            error_code: error
                .data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            error_message: Some(error.message.to_string()),
            ..SessionShellCleanupReport::default()
        },
    }
}

async fn cleanup_session_cdp_targets(
    m3_state: &SharedM3State,
    cdp_target_owners: &SharedCdpTargetOwners,
    session_id: &str,
) -> SessionCdpCleanupReport {
    let mut owned = match remove_session_cdp_target_owners(cdp_target_owners, session_id) {
        Ok(owned) => owned,
        Err(detail) => {
            tracing::error!(
                code = error_codes::TOOL_INTERNAL_ERROR,
                session_id,
                detail = %detail,
                "session lifecycle could not lock CDP target ownership registry"
            );
            return SessionCdpCleanupReport {
                failed: 1,
                error_messages: vec![detail],
                ..SessionCdpCleanupReport::default()
            };
        }
    };
    let memory_owned_before = owned.len();
    let persisted = match super::session_continuity::read_persisted_cdp_target_owners_for_session(
        m3_state, session_id,
    ) {
        Ok(persisted) => persisted,
        Err(detail) => {
            tracing::error!(
                code = error_codes::STORAGE_CORRUPTED,
                session_id,
                detail = %detail,
                "session lifecycle could not read persisted CDP target owner rows"
            );
            return SessionCdpCleanupReport {
                memory_owned_before,
                owned_before: memory_owned_before,
                failed: 1,
                target_ids: owned
                    .iter()
                    .map(|(_owner_key, owner)| owner.cdp_target_id.clone())
                    .collect(),
                error_messages: vec![detail],
                ..SessionCdpCleanupReport::default()
            };
        }
    };
    let persisted_owned_before = persisted.len();
    let mut seen = owned
        .iter()
        .map(|(owner_key, owner)| {
            cdp_cleanup_owner_identity(owner_key, &owner.endpoint, &owner.cdp_target_id)
        })
        .collect::<BTreeSet<_>>();
    for (owner_key, row) in persisted {
        let identity =
            cdp_cleanup_owner_identity(&owner_key, &row.owner.endpoint, &row.owner.cdp_target_id);
        if seen.insert(identity) {
            owned.push((owner_key, row.owner));
        }
    }
    let mut report = SessionCdpCleanupReport {
        memory_owned_before,
        persisted_owned_before,
        owned_before: owned.len(),
        target_ids: owned
            .iter()
            .map(|(_owner_key, owner)| owner.cdp_target_id.clone())
            .collect(),
        ..SessionCdpCleanupReport::default()
    };
    for (owner_key, owner) in owned {
        match close_cdp_target_for_cleanup(&owner.cdp_target_id, &owner).await {
            Ok(close_outcome) => {
                match super::session_continuity::delete_persisted_cdp_target_owner_row(
                    m3_state,
                    &owner_key,
                    &owner.cdp_target_id,
                ) {
                    Ok(deleted) => {
                        if deleted {
                            report.persisted_rows_deleted =
                                report.persisted_rows_deleted.saturating_add(1);
                        }
                    }
                    Err(detail) => {
                        report.failed = report.failed.saturating_add(1);
                        report.error_messages.push(detail.clone());
                        tracing::error!(
                            code = error_codes::STORAGE_CORRUPTED,
                            session_id,
                            hwnd = owner.window_hwnd,
                            endpoint = %owner.endpoint,
                            cdp_target_id = %owner.cdp_target_id,
                            detail = %detail,
                            "session lifecycle closed CDP target but failed to delete persisted owner row"
                        );
                        continue;
                    }
                }
                match close_outcome {
                    CdpCleanupCloseOutcome::Closed => {
                        report.closed = report.closed.saturating_add(1);
                    }
                    CdpCleanupCloseOutcome::AlreadyAbsent => {
                        report.already_absent = report.already_absent.saturating_add(1);
                    }
                    CdpCleanupCloseOutcome::EndpointUnreachable => {
                        report.endpoint_unreachable = report.endpoint_unreachable.saturating_add(1);
                    }
                }
                tracing::info!(
                    code = "MCP_SESSION_CDP_TARGET_CLEANUP",
                    session_id,
                    hwnd = owner.window_hwnd,
                    endpoint = %owner.endpoint,
                    cdp_target_id = %owner.cdp_target_id,
                    close_outcome = close_outcome.as_str(),
                    "readback=Target.closeTarget edge=session_cleanup after=resolved"
                );
            }
            Err(detail) => {
                report.failed = report.failed.saturating_add(1);
                report.error_messages.push(detail.clone());
                tracing::error!(
                    code = error_codes::A11Y_CDP_AXTREE_FAILED,
                    session_id,
                    hwnd = owner.window_hwnd,
                    endpoint = %owner.endpoint,
                    cdp_target_id = %owner.cdp_target_id,
                    detail = %detail,
                    "session lifecycle removed CDP owner but failed to close target"
                );
            }
        }
    }
    report
}

fn cdp_cleanup_owner_identity(owner_key: &str, endpoint: &str, target_id: &str) -> String {
    format!(
        "{}|{}|{}",
        owner_key.trim(),
        endpoint.trim(),
        target_id.trim().to_ascii_lowercase()
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CdpCleanupCloseOutcome {
    Closed,
    AlreadyAbsent,
    EndpointUnreachable,
}

impl CdpCleanupCloseOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::AlreadyAbsent => "already_absent",
            Self::EndpointUnreachable => "endpoint_unreachable",
        }
    }
}

fn remove_session_cdp_target_owners(
    cdp_target_owners: &SharedCdpTargetOwners,
    session_id: &str,
) -> Result<Vec<(String, CdpTargetOwner)>, String> {
    let mut guard = cdp_target_owners
        .lock()
        .map_err(|_error| "CDP target ownership registry lock poisoned".to_owned())?;
    let mut owned = Vec::new();
    guard.retain(|target_id, owner| {
        if owner.session_id == session_id {
            owned.push((target_id.clone(), owner.clone()));
            false
        } else {
            true
        }
    });
    Ok(owned)
}

/// Upper bound for closing one owned CDP target during teardown (#210). The
/// attach handshake has its own bound; this also covers a browser that accepts
/// the connection and then never answers `Target.closeTarget`.
const CDP_CLEANUP_CLOSE_TIMEOUT: Duration = Duration::from_secs(15);

async fn close_cdp_target_for_cleanup(
    target_id: &str,
    owner: &CdpTargetOwner,
) -> Result<CdpCleanupCloseOutcome, String> {
    close_cdp_target_bounded(target_id, owner, CDP_CLEANUP_CLOSE_TIMEOUT).await
}

/// A timeout is an error, never a reclaim: the owner row stays so the retry
/// path closes the tab once the browser answers again.
async fn close_cdp_target_bounded(
    target_id: &str,
    owner: &CdpTargetOwner,
    limit: Duration,
) -> Result<CdpCleanupCloseOutcome, String> {
    tokio::time::timeout(limit, close_cdp_target_once(target_id, owner))
        .await
        .unwrap_or_else(|_elapsed| {
            Err(format!(
                "closing CDP target {target_id:?} on {:?} timed out after {} ms",
                owner.endpoint,
                limit.as_millis()
            ))
        })
}

#[cfg(windows)]
async fn close_cdp_target_once(
    target_id: &str,
    owner: &CdpTargetOwner,
) -> Result<CdpCleanupCloseOutcome, String> {
    if is_chrome_bridge_endpoint(&owner.endpoint) {
        return crate::chrome_debugger_bridge::close_tab(owner.window_hwnd, target_id)
            .await
            .map(|_closed| CdpCleanupCloseOutcome::Closed)
            .or_else(|error| {
                if chrome_bridge_close_target_already_absent(error.detail(), target_id) {
                    Ok(CdpCleanupCloseOutcome::AlreadyAbsent)
                } else {
                    Err(error.detail().to_owned())
                }
            });
    }
    synapse_a11y::cdp_close_target(&owner.endpoint, target_id)
        .await
        .map(|_closed| CdpCleanupCloseOutcome::Closed)
        .or_else(|error| raw_cdp_close_error_outcome(error, owner))
}

/// Maps a raw-CDP `Target.closeTarget` failure to a cleanup outcome. A tab the
/// user already closed (#211) and an unreachable owner endpoint both reclaim the
/// owner row; anything else is a real failure that keeps the row for retry.
#[cfg(windows)]
fn raw_cdp_close_error_outcome(
    error: synapse_a11y::A11yError,
    owner: &CdpTargetOwner,
) -> Result<CdpCleanupCloseOutcome, String> {
    if synapse_a11y::is_cdp_close_target_absent(&error) {
        tracing::info!(
            hwnd = owner.window_hwnd,
            endpoint = %owner.endpoint,
            cdp_target_id = %owner.cdp_target_id,
            "session lifecycle found the raw-CDP target already closed; reclaiming owner row"
        );
        return Ok(CdpCleanupCloseOutcome::AlreadyAbsent);
    }
    match error {
        synapse_a11y::A11yError::CdpUnreachable { detail } => {
            tracing::warn!(
                code = error_codes::A11Y_CDP_UNREACHABLE,
                hwnd = owner.window_hwnd,
                endpoint = %owner.endpoint,
                cdp_target_id = %owner.cdp_target_id,
                detail = %detail,
                "session lifecycle reclaimed persisted CDP owner row after owner endpoint became unreachable"
            );
            Ok(CdpCleanupCloseOutcome::EndpointUnreachable)
        }
        other => Err(other.to_string()),
    }
}

/// Normal-bridge tab-lane endpoint of any declared browser (legacy
/// `chrome-extension://<id>/chrome.tabs|chrome.debugger` rows included).
#[cfg(windows)]
fn is_chrome_bridge_endpoint(endpoint: &str) -> bool {
    synapse_browsers::BrowserRegistry::global().is_bridge_tab_lane_endpoint(endpoint)
}

#[cfg(windows)]
fn chrome_bridge_close_target_already_absent(detail: &str, target_id: &str) -> bool {
    detail.contains("targetIdHint")
        && detail.contains(target_id)
        && detail.contains("did not match any chrome.tabs tab id")
}

#[cfg(not(windows))]
async fn close_cdp_target_once(
    target_id: &str,
    owner: &CdpTargetOwner,
) -> Result<CdpCleanupCloseOutcome, String> {
    Err(format!(
        "CDP target cleanup is only available on Windows; target_id={target_id:?} endpoint={:?}",
        owner.endpoint
    ))
}

fn owned_input_count(snapshot: &synapse_action::SessionInputSnapshot, session_id: &str) -> usize {
    snapshot
        .sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .map_or(0, |session| {
            session.keys.len() + session.mouse_buttons.len() + session.pads.len()
        })
}

fn add_if_stale(
    candidates: &mut BTreeMap<String, &'static str>,
    active_sessions: &BTreeSet<String>,
    session_id: &str,
    reason: &'static str,
) {
    if !active_sessions.contains(session_id) {
        candidates.entry(session_id.to_owned()).or_insert(reason);
    }
}

/// The OS pids that prove a spawned agent is alive.
///
/// The inner agent process can exit before the launcher wrapper writes the
/// terminal `completion-status.json`. Treat either process as live so cleanup
/// cannot race ahead of the wrapper's final artifact.
fn spawned_agent_probe_pids(read: &super::session_registry::SessionRegistryRead) -> Vec<u32> {
    if read.closed_at_unix_ms.is_some() {
        return Vec::new();
    }
    let Some(spawned_agent) = read.spawned_agent.as_ref() else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    if let Some(agent_process_id) = spawned_agent.agent_process_id
        && agent_process_id != 0
    {
        pids.push(agent_process_id);
    }
    if spawned_agent.launcher_process_id != 0 && !pids.contains(&spawned_agent.launcher_process_id)
    {
        pids.push(spawned_agent.launcher_process_id);
    }
    pids
}

/// Partitions spawned-agent registry sessions by a real liveness probe (#1238).
///
/// Returns `(live, exited)`: `live` is the set of session ids whose spawned
/// agent or launcher process is still running. These are protected from
/// idle-driven teardown so the daemon never kills an agent that is mid-work but
/// quiet on the MCP wire. `exited` are sessions whose spawned processes are
/// gone, still reap candidates even if no exit event reached the journal.
/// `process_alive` is injected so the rule is unit-testable without real
/// processes; production passes the OS process-table probe.
fn partition_spawned_sessions<'a>(
    reads: &'a [super::session_registry::SessionRegistryRead],
    process_alive: &dyn Fn(u32) -> bool,
) -> (
    BTreeSet<String>,
    Vec<&'a super::session_registry::SessionRegistryRead>,
) {
    let mut live = BTreeSet::new();
    let mut exited = Vec::new();
    for read in reads {
        let pids = spawned_agent_probe_pids(read);
        if pids.is_empty() {
            continue;
        }
        if pids.iter().any(|pid| process_alive(*pid)) {
            live.insert(read.session_id.clone());
        } else {
            exited.push(read);
        }
    }
    (live, exited)
}

fn live_spawned_session_ids(
    reads: &[super::session_registry::SessionRegistryRead],
    process_alive: &dyn Fn(u32) -> bool,
) -> BTreeSet<String> {
    partition_spawned_sessions(reads, process_alive).0
}

fn spawned_agent_exit_reason(read: &super::session_registry::SessionRegistryRead) -> &'static str {
    let Some(spawned) = read.spawned_agent.as_ref() else {
        return SPAWNED_AGENT_PROCESS_EXITED_REASON;
    };
    if spawned_agent_completion_status(&spawned.log_dir).as_deref() == Some("ok") {
        SPAWN_COMPLETED_REASON
    } else {
        SPAWNED_AGENT_PROCESS_EXITED_REASON
    }
}

fn session_target_shutdown_readback(target: &SessionTarget) -> SessionTargetShutdownReadback {
    match target {
        SessionTarget::Window { hwnd } => SessionTargetShutdownReadback::Window { hwnd: *hwnd },
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => SessionTargetShutdownReadback::Cdp {
            window_hwnd: *window_hwnd,
            cdp_target_id: cdp_target_id.clone(),
        },
    }
}

fn session_cdp_target_owner_readback(
    owner_key: String,
    owner: CdpTargetOwner,
) -> SessionCdpTargetOwnerReadback {
    SessionCdpTargetOwnerReadback {
        owner_key,
        session_id: owner.session_id,
        window_hwnd: owner.window_hwnd,
        endpoint: owner.endpoint,
        cdp_target_id: owner.cdp_target_id,
        requested_url: redact_url_for_public_readback(&owner.requested_url),
        target_url: redact_url_for_public_readback(&owner.target_url),
        created_at_unix_ms: owner.created_at_unix_ms,
    }
}

fn persisted_cdp_target_owner_readbacks_for_session(
    m3_state: &SharedM3State,
    session_id: &str,
) -> Result<Vec<SessionCdpTargetOwnerReadback>, String> {
    let mut owners = super::session_continuity::read_persisted_cdp_target_owners_for_session(
        m3_state, session_id,
    )?
    .into_iter()
    .map(|(owner_key, row)| {
        let mut readback = session_cdp_target_owner_readback(owner_key, row.owner);
        readback.session_id = row.owner_session_id;
        readback
    })
    .collect::<Vec<_>>();
    owners.sort_by(|left, right| left.owner_key.cmp(&right.owner_key));
    Ok(owners)
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn reconciled_teardown_exit_reason(
    reason: &str,
    processes: &SessionProcessCleanupReport,
) -> String {
    if reason == SPAWNED_AGENT_PROCESS_EXITED_REASON && cleanup_observed_ok_completion(processes) {
        SPAWN_COMPLETED_REASON.to_owned()
    } else {
        reason.to_owned()
    }
}

fn cleanup_observed_ok_completion(processes: &SessionProcessCleanupReport) -> bool {
    processes.items.iter().any(|item| {
        item.tool == ACT_SPAWN_AGENT_TOOL_NAME
            && item
                .completion_status_path
                .as_deref()
                .and_then(|path| completion_status_from_file(Path::new(path)))
                .as_deref()
                == Some("ok")
    })
}

fn spawned_agent_completion_status(log_dir: &str) -> Option<String> {
    completion_status_from_file(&Path::new(log_dir).join("completion-status.json"))
}

fn completion_status_from_file(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let status = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    status
        .get("status")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

fn session_store_db(m3_state: &SharedM3State) -> Result<Arc<Db>, String> {
    let mut state = m3_state.lock().map_err(|_error| {
        "M3 service state lock poisoned during session-store cleanup".to_owned()
    })?;
    state
        .ensure_storage()
        .map_err(|error| format!("open storage for session-store cleanup: {error}"))
}

fn session_store_row_exists(db: &Db, key: &[u8]) -> Result<bool, String> {
    db.scan_cf_prefix(cf::CF_KV, key)
        .map_err(|error| error.to_string())
        .map(|rows| {
            rows.into_iter()
                .any(|(row_key, _value)| row_key.as_slice() == key)
        })
}

fn validate_lifecycle_session_id(session_id: &str) -> Result<(), ErrorData> {
    if session_id.trim().is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "session id must not be empty",
        ));
    }
    if session_id.chars().count() > 512 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "session id must be at most 512 Unicode scalar values",
        ));
    }
    if !session_id.chars().all(|ch| ('!'..='~').contains(&ch)) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "session id must contain only visible ASCII characters",
        ));
    }
    Ok(())
}

fn session_teardown_error(report: SessionTeardownReport) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "session teardown for {:?} failed with {} cleanup failure(s)",
            report.session_id, report.failure_count
        ),
        Some(json!({
            "code": error_codes::TOOL_INTERNAL_ERROR,
            "report": report,
        })),
    )
}

#[cfg(test)]
#[path = "tests/session_dead_spawn_owner_75.rs"]
mod dead_spawn_owner_tests_75;

#[cfg(debug_assertions)]
#[path = "session_cleanup_fault_hook.rs"]
mod cleanup_fault_hook;

#[path = "session_teardown_gate.rs"]
mod teardown_gate;

#[cfg(test)]
#[path = "tests/session_cleanup_fault_hook_75.rs"]
mod cleanup_fault_hook_75_tests;

#[cfg(test)]
#[path = "tests/session_failed_process_retain_75.rs"]
mod failed_process_retain_tests_75;

#[cfg(test)]
#[path = "tests/expired_lease_journal_95.rs"]
mod expired_lease_journal_95_tests;

#[cfg(test)]
#[path = "tests/terminated_sessions_eviction_14.rs"]
mod terminated_sessions_eviction_14_tests;

#[cfg(test)]
#[path = "tests/session_cdp_close_absent_211.rs"]
mod session_cdp_close_absent_211_tests;

#[cfg(test)]
#[path = "tests/session_teardown_off_worker_212.rs"]
mod session_teardown_off_worker_212_tests;

#[cfg(test)]
#[path = "tests/session_teardown_serialized_213.rs"]
mod session_teardown_serialized_213_tests;

#[cfg(test)]
#[path = "tests/session_cdp_close_timeout_210.rs"]
mod session_cdp_close_timeout_210_tests;
