use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use synapse_core::SubsystemHealth;

use self::ledger_rotation::LedgerRotation;
use crate::process_probe::{ProcessProbe, probe_process};

const SCHEMA_VERSION: u32 = 1;
const RUN_CURRENT_FILE: &str = "daemon-run-current.json";
const TOOL_LAST_FILE: &str = "daemon-tool-last.json";
const TOOL_EVENTS_FILE: &str = "daemon-tool-events.jsonl";
const EXIT_EVENTS_FILE: &str = "daemon-exit.jsonl";

static STATE: OnceLock<Mutex<Option<DaemonLifecycleState>>> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

#[derive(Clone, Debug)]
pub(crate) struct DaemonLifecycleConfig {
    pub mode: &'static str,
    pub bind_addr: Option<String>,
    pub db_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "explicit source-of-truth path names make health and error evidence unambiguous"
)]
pub(crate) struct DaemonLifecyclePaths {
    pub db_path: String,
    pub run_current_path: String,
    pub tool_last_path: String,
    pub tool_events_path: String,
    pub exit_events_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RunRecord {
    schema_version: u32,
    run_id: String,
    pid: u32,
    #[serde(default)]
    process_started_at_100ns: Option<u64>,
    mode: String,
    bind_addr: Option<String>,
    db_path: String,
    started_at_unix_ms: u64,
    ended_at_unix_ms: Option<u64>,
    ended_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ToolCallStart {
    pub tool: String,
    pub mcp_session_id: Option<String>,
    pub audit_context: Option<Value>,
    pub audit_context_read_error: Option<Value>,
    pub foreground: Option<Value>,
    pub foreground_read_error: Option<Value>,
    pub session_target: Option<Value>,
    pub session_target_read_error: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct InFlightToolCallRead {
    pub seq: u64,
    pub tool: String,
    pub mcp_session_id: Option<String>,
    pub started_at_unix_ms: u64,
    pub elapsed_ms: u64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ToolEvent {
    schema_version: u32,
    run_id: String,
    pid: u32,
    seq: u64,
    event_kind: String,
    tool: String,
    status: String,
    started_at_unix_ms: u64,
    finished_at_unix_ms: Option<u64>,
    duration_ms: Option<u64>,
    mcp_session_id: Option<String>,
    audit_context: Option<Value>,
    audit_context_read_error: Option<Value>,
    foreground: Option<Value>,
    foreground_read_error: Option<Value>,
    session_target: Option<Value>,
    session_target_read_error: Option<Value>,
    effective_target: Option<Value>,
    error: Option<Value>,
    panic: Option<Value>,
    detail: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExitEvent {
    schema_version: u32,
    run_id: String,
    pid: u32,
    event_kind: String,
    cause: String,
    detail: Value,
    recorded_at_unix_ms: u64,
    run: Option<RunRecord>,
    last_tool_event: Option<ToolEvent>,
    in_flight_tool_events: Vec<ToolEvent>,
    paths: DaemonLifecyclePaths,
}

#[derive(Clone, Debug)]
struct DaemonLifecycleState {
    run: RunRecord,
    paths: DaemonLifecyclePaths,
    in_flight: BTreeMap<u64, ToolEvent>,
    seq: u64,
    last_error: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ToolCallGuard {
    /// `0` is the detached guard: the ledger could not record the start, so
    /// finishing it is a no-op (real event sequence numbers start at 1).
    seq: u64,
}

/// Log-rate limiter for diagnostic-ledger failures (#172): a broken ledger
/// fails on every tool call, so only one line per interval is emitted and the
/// number of suppressed failures is reported on the next line.
pub(crate) struct FailureLogLimiter {
    interval_ms: u64,
    last_logged_ms: AtomicU64,
    suppressed: AtomicU64,
}

impl FailureLogLimiter {
    const NEVER: u64 = u64::MAX;

    pub(crate) const fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            last_logged_ms: AtomicU64::new(Self::NEVER),
            suppressed: AtomicU64::new(0),
        }
    }

    /// `Some(suppressed_since_last_line)` when this failure should be logged.
    pub(crate) fn admit(&self, now_ms: u64) -> Option<u64> {
        let last = self.last_logged_ms.load(Ordering::Relaxed);
        if last != Self::NEVER && now_ms.saturating_sub(last) < self.interval_ms {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.last_logged_ms.store(now_ms, Ordering::Relaxed);
        Some(self.suppressed.swap(0, Ordering::Relaxed))
    }
}

/// One ledger-failure log line per 30 s; tool results are never affected.
static LEDGER_FAILURE_LOG: FailureLogLimiter = FailureLogLimiter::new(30_000);

/// Logs (rate-limited) a failed diagnostic-ledger operation. The caller keeps
/// going with the real tool result (#172).
pub(crate) fn report_ledger_failure(operation: &str, tool: &str, error: &anyhow::Error) {
    if let Some(suppressed) = LEDGER_FAILURE_LOG.admit(now_unix_ms()) {
        tracing::error!(
            code = "MCP_DAEMON_LIFECYCLE_WRITE_FAILED",
            operation,
            tool,
            suppressed_since_last_log = suppressed,
            detail = format!("{error:#}"),
            "daemon lifecycle diagnostic ledger failed; tool result unaffected"
        );
    }
}

#[derive(Debug)]
pub(crate) struct ContextEvent {
    pub event_kind: &'static str,
    pub tool: &'static str,
    pub status: &'static str,
    pub mcp_session_id: Option<String>,
    pub foreground: Option<Value>,
    pub foreground_read_error: Option<Value>,
    pub detail: Value,
}

pub(crate) fn configure(config: DaemonLifecycleConfig) -> anyhow::Result<DaemonLifecyclePaths> {
    fs::create_dir_all(&config.db_path).with_context(|| {
        format!(
            "create daemon lifecycle db directory {}",
            config.db_path.display()
        )
    })?;
    let paths = DaemonLifecyclePaths {
        db_path: config.db_path.display().to_string(),
        run_current_path: config.db_path.join(RUN_CURRENT_FILE).display().to_string(),
        tool_last_path: config.db_path.join(TOOL_LAST_FILE).display().to_string(),
        tool_events_path: config.db_path.join(TOOL_EVENTS_FILE).display().to_string(),
        exit_events_path: config.db_path.join(EXIT_EVENTS_FILE).display().to_string(),
    };

    let previous_run = read_previous_diagnostic::<RunRecord>(Path::new(&paths.run_current_path));
    let previous_last_tool =
        read_previous_diagnostic::<ToolEvent>(Path::new(&paths.tool_last_path));

    let run = RunRecord {
        schema_version: SCHEMA_VERSION,
        run_id: format!("{}-{}", now_unix_ms(), std::process::id()),
        pid: std::process::id(),
        process_started_at_100ns: match probe_process(std::process::id()) {
            ProcessProbe::Running { created_100ns } => Some(created_100ns),
            ProcessProbe::Gone => None,
            ProcessProbe::Unknown(error) => {
                tracing::warn!(code = "MCP_DAEMON_PROCESS_IDENTITY_UNKNOWN", %error,
                    "cannot record process creation time");
                None
            }
        },
        mode: config.mode.to_owned(),
        bind_addr: config.bind_addr,
        db_path: paths.db_path.clone(),
        started_at_unix_ms: now_unix_ms(),
        ended_at_unix_ms: None,
        ended_reason: None,
    };

    let previous_has_exit = match previous_run.as_ref() {
        Some(previous) if previous.ended_at_unix_ms.is_none() => {
            match has_exit_record(Path::new(&paths.exit_events_path), &previous.run_id) {
                Ok(found) => Some(found),
                Err(error) => {
                    tracing::warn!(
                        code = "MCP_DAEMON_PREVIOUS_EXIT_UNKNOWN", %error,
                        previous_run_id = %previous.run_id,
                        "cannot read previous exit ledger; continuing startup with unknown history"
                    );
                    None
                }
            }
        }
        _ => Some(false),
    };
    if let Some((previous, last_seen_unix_ms, liveness)) = previous_has_exit.and_then(|has_exit| {
        unclean_previous_run(previous_run.as_ref(), previous_last_tool.as_ref(), has_exit)
    }) {
        // #158: best-effort cause from the diagnostic heartbeat file.
        let heartbeat = crate::daemon_heartbeat::assess(
            Path::new(&paths.db_path),
            &previous.run_id,
            last_seen_unix_ms,
            now_unix_ms(),
            crate::daemon_heartbeat::boot_unix_ms(),
        );
        let suspected = heartbeat.suspected.as_str();
        let (event_kind, cause, reason) = unclean_event_labels(liveness);
        tracing::warn!(
            code = "MCP_DAEMON_PREVIOUS_RUN_UNCLEAN",
            previous_pid = previous.pid,
            previous_run_id = %previous.run_id,
            last_seen_unix_ms,
            last_heartbeat_age_ms = ?heartbeat.last_heartbeat_age_ms,
            suspected,
            reason = "unknown",
            event_kind,
            "previous daemon run ended without an exit record"
        );
        append_json_line(
            Path::new(&paths.exit_events_path),
            &ExitEvent {
                schema_version: SCHEMA_VERSION,
                run_id: previous.run_id.clone(),
                pid: previous.pid,
                event_kind: event_kind.to_owned(),
                cause: cause.to_owned(),
                detail: json!({
                    "new_pid": std::process::id(),
                    "new_run_id": run.run_id,
                    "last_seen_unix_ms": last_seen_unix_ms,
                    "last_heartbeat_age_ms": heartbeat.last_heartbeat_age_ms,
                    "suspected": suspected,
                    "reason": reason,
                }),
                recorded_at_unix_ms: now_unix_ms(),
                run: Some(previous.clone()),
                last_tool_event: previous_last_tool.clone(),
                in_flight_tool_events: previous_last_tool
                    .iter()
                    .filter(|event| event.status == "started")
                    .cloned()
                    .collect(),
                paths: paths.clone(),
            },
        )
        .with_context(|| {
            format!(
                "append previous unclean daemon exit event {}",
                paths.exit_events_path
            )
        })?;
    }

    if let Err(error) = write_json_atomic(Path::new(&paths.run_current_path), &run) {
        tracing::warn!(
            code = "MCP_DAEMON_CURRENT_RUN_WRITE_FAILED",
            path = %paths.run_current_path,
            detail = %error,
            "cannot write diagnostic current run; continuing startup"
        );
    }
    let state = DaemonLifecycleState {
        run,
        paths: paths.clone(),
        in_flight: BTreeMap::new(),
        seq: 0,
        last_error: None,
    };
    let slot = state_slot();
    let mut guard = slot
        .lock()
        .map_err(|_error| anyhow::anyhow!("daemon lifecycle state lock poisoned"))?;
    *guard = Some(state);
    tracing::info!(
        code = "MCP_DAEMON_LIFECYCLE_CONFIGURED",
        run_current_path = %paths.run_current_path,
        tool_last_path = %paths.tool_last_path,
        tool_events_path = %paths.tool_events_path,
        exit_events_path = %paths.exit_events_path,
        "daemon lifecycle ledger configured"
    );
    Ok(paths)
}

/// What the startup probe could establish about the previous run's process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviousLiveness {
    Alive,
    Dead,
    /// The process could not be queried (ACCESS_DENIED on a reused PID, ...).
    Unknown,
}

fn unclean_previous_run<'a>(
    previous: Option<&'a RunRecord>,
    last_tool: Option<&ToolEvent>,
    has_exit: bool,
) -> Option<(&'a RunRecord, u64, PreviousLiveness)> {
    unclean_previous_run_with(
        previous,
        last_tool,
        has_exit,
        crate::daemon_heartbeat::boot_unix_ms(),
        crate::process_probe::probe_process,
    )
}

fn unclean_previous_run_with<'a>(
    previous: Option<&'a RunRecord>,
    last_tool: Option<&ToolEvent>,
    has_exit: bool,
    boot_unix_ms: Option<u64>,
    probe: impl FnOnce(u32) -> ProcessProbe,
) -> Option<(&'a RunRecord, u64, PreviousLiveness)> {
    let previous = previous?;
    if previous.ended_at_unix_ms.is_some() || has_exit {
        return None;
    }
    let liveness = previous_liveness(previous, boot_unix_ms, probe);
    if liveness == PreviousLiveness::Alive {
        return None;
    }
    let last_seen = last_tool
        .filter(|event| event.run_id == previous.run_id)
        .map_or(previous.started_at_unix_ms, |event| {
            event
                .finished_at_unix_ms
                .unwrap_or(event.started_at_unix_ms)
                .max(previous.started_at_unix_ms)
        });
    Some((previous, last_seen, liveness))
}

/// A run that started before the machine booted cannot still be running, so a
/// PID that is now owned by a process we cannot open (reused by SYSTEM after a
/// reboot) is still reported dead. Otherwise an unqueryable process is
/// `Unknown`, never silently treated as alive (#144).
fn previous_liveness(
    previous: &RunRecord,
    boot_unix_ms: Option<u64>,
    probe: impl FnOnce(u32) -> ProcessProbe,
) -> PreviousLiveness {
    if boot_unix_ms.is_some_and(|boot| previous.started_at_unix_ms < boot) {
        return PreviousLiveness::Dead;
    }
    match probe(previous.pid) {
        ProcessProbe::Running { created_100ns } => {
            if previous
                .process_started_at_100ns
                .is_none_or(|expected| expected == created_100ns)
            {
                PreviousLiveness::Alive
            } else {
                PreviousLiveness::Dead
            }
        }
        ProcessProbe::Gone => PreviousLiveness::Dead,
        ProcessProbe::Unknown(error) => {
            tracing::warn!(code = "MCP_DAEMON_PREVIOUS_PROCESS_UNKNOWN", %error,
                previous_pid = previous.pid, "cannot determine previous process liveness");
            PreviousLiveness::Unknown
        }
    }
}

/// Ledger `(event_kind, cause, reason)` for a non-alive previous run.
fn unclean_event_labels(liveness: PreviousLiveness) -> (&'static str, &'static str, &'static str) {
    match liveness {
        PreviousLiveness::Unknown => (
            "previous_run_unknown",
            "previous_process_unverifiable",
            "previous process could not be queried and no exit was recorded; its PID may have been reused",
        ),
        PreviousLiveness::Alive | PreviousLiveness::Dead => (
            "previous_run_unclean",
            "process_missing_on_startup",
            "previous process is absent or its PID was reused, with no recorded exit",
        ),
    }
}

fn has_exit_record(path: &Path, run_id: &str) -> anyhow::Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("read previous exit ledger"),
    };
    for line in io::BufReader::new(file).lines() {
        let line = line.context("read previous exit ledger line")?;
        // A killed writer may leave a partial final line. It is not an exit.
        if let Ok(event) = serde_json::from_str::<ExitEvent>(&line)
            && event.run_id == run_id
            && event.event_kind == "daemon_exit"
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[path = "daemon_ledger_rotation.rs"]
mod ledger_rotation;
#[cfg(test)]
#[path = "tests/daemon_lifecycle_liveness_144.rs"]
mod liveness_144_tests;
#[cfg(test)]
#[path = "daemon_lifecycle_shutdown_tests.rs"]
mod shutdown_tests;
#[cfg(test)]
#[path = "tests/tool_ledger_failure_172.rs"]
mod tool_ledger_failure_172_tests;

pub(crate) fn install_panic_hook() {
    PANIC_HOOK_INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Err(error) = record_panic(info) {
                eprintln!("synapse-mcp daemon lifecycle panic record failed: {error:#}");
            }
            previous(info);
        }));
    });
}

pub(crate) fn begin_tool_call(start: ToolCallStart) -> anyhow::Result<ToolCallGuard> {
    let slot = state_slot();
    let mut guard = slot
        .lock()
        .map_err(|_error| anyhow::anyhow!("daemon lifecycle state lock poisoned"))?;
    let Some(state) = guard.as_mut() else {
        bail!("daemon lifecycle ledger is not configured");
    };
    state.seq = state.seq.saturating_add(1);
    let seq = state.seq;
    let event = ToolEvent {
        schema_version: SCHEMA_VERSION,
        run_id: state.run.run_id.clone(),
        pid: state.run.pid,
        seq,
        event_kind: "tool_call".to_owned(),
        tool: start.tool,
        status: "started".to_owned(),
        started_at_unix_ms: now_unix_ms(),
        finished_at_unix_ms: None,
        duration_ms: None,
        mcp_session_id: start.mcp_session_id,
        audit_context: start.audit_context,
        audit_context_read_error: start.audit_context_read_error,
        foreground: start.foreground,
        foreground_read_error: start.foreground_read_error,
        session_target: start.session_target,
        session_target_read_error: start.session_target_read_error,
        effective_target: None,
        error: None,
        panic: None,
        detail: None,
    };
    // #172: a ledger write failure is recorded in `last_error` and logged, but
    // the call still runs and is still tracked in memory.
    write_tool_event(state, &event);
    state.in_flight.insert(seq, event);
    Ok(ToolCallGuard { seq })
}

/// Infallible `begin_tool_call` for the MCP handler (#172): diagnostics must
/// never refuse a tool call. If the ledger is unusable the call proceeds with
/// a detached guard.
pub(crate) fn begin_tool_call_best_effort(start: ToolCallStart) -> ToolCallGuard {
    let tool = start.tool.clone();
    begin_tool_call(start).unwrap_or_else(|error| {
        report_ledger_failure("begin_tool_call", &tool, &error);
        ToolCallGuard { seq: 0 }
    })
}

pub(crate) fn record_context_event(input: ContextEvent) -> anyhow::Result<u64> {
    let slot = state_slot();
    let mut guard = slot
        .lock()
        .map_err(|_error| anyhow::anyhow!("daemon lifecycle state lock poisoned"))?;
    let Some(state) = guard.as_mut() else {
        bail!("daemon lifecycle ledger is not configured");
    };
    state.seq = state.seq.saturating_add(1);
    let seq = state.seq;
    let recorded_at_unix_ms = now_unix_ms();
    let event = ToolEvent {
        schema_version: SCHEMA_VERSION,
        run_id: state.run.run_id.clone(),
        pid: state.run.pid,
        seq,
        event_kind: input.event_kind.to_owned(),
        tool: input.tool.to_owned(),
        status: input.status.to_owned(),
        started_at_unix_ms: recorded_at_unix_ms,
        finished_at_unix_ms: Some(recorded_at_unix_ms),
        duration_ms: Some(0),
        mcp_session_id: input.mcp_session_id,
        audit_context: None,
        audit_context_read_error: None,
        foreground: input.foreground,
        foreground_read_error: input.foreground_read_error,
        session_target: None,
        session_target_read_error: None,
        effective_target: None,
        error: None,
        panic: None,
        detail: Some(input.detail),
    };
    write_tool_event(state, &event);
    Ok(seq)
}

impl ToolCallGuard {
    pub(crate) fn finish_ok_with_effective_target(
        self,
        effective_target: Option<Value>,
    ) -> anyhow::Result<()> {
        finish_tool_call(self.seq, "ok", None, None, effective_target)
    }

    pub(crate) fn finish_error(self, error: Value) -> anyhow::Result<()> {
        finish_tool_call(self.seq, "error", Some(error), None, None)
    }

    pub(crate) fn finish_error_with_effective_target(
        self,
        error: Value,
        effective_target: Option<Value>,
    ) -> anyhow::Result<()> {
        finish_tool_call(self.seq, "error", Some(error), None, effective_target)
    }

    pub(crate) fn finish_panic(self, panic: Value) -> anyhow::Result<()> {
        finish_tool_call(self.seq, "panic", None, Some(panic), None)
    }
}

pub(crate) fn record_graceful_exit(source: &'static str) -> anyhow::Result<()> {
    record_exit(
        "daemon_exit",
        "graceful",
        json!({
            "source": source,
        }),
    )
}

pub(crate) fn record_startup_exit(cause: &'static str, detail: Value) -> anyhow::Result<()> {
    record_exit("daemon_exit", cause, detail)
}

pub(crate) fn record_top_level_error(detail: &str) -> anyhow::Result<()> {
    record_exit(
        "daemon_exit",
        "top_level_error",
        json!({
            "error": detail,
        }),
    )
}

pub(crate) fn health_subsystem() -> SubsystemHealth {
    let slot = state_slot();
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(_error) => {
            return SubsystemHealth {
                status: "error".to_owned(),
                detail: Some("daemon lifecycle state lock poisoned".to_owned()),
                ..SubsystemHealth::default()
            };
        }
    };
    let Some(state) = guard.as_ref() else {
        return SubsystemHealth {
            status: "not_configured".to_owned(),
            detail: Some("daemon lifecycle ledger not configured in this process".to_owned()),
            ..SubsystemHealth::default()
        };
    };
    let status = if state.last_error.is_some() {
        "error"
    } else {
        "ok"
    };
    SubsystemHealth {
        status: status.to_owned(),
        detail: Some(health_detail_for_state(state)),
        ..SubsystemHealth::default()
    }
}

pub(crate) fn diagnostic_value() -> Value {
    let slot = state_slot();
    let Ok(guard) = slot.lock() else {
        return json!({
            "status": "error",
            "detail": "daemon lifecycle state lock poisoned",
        });
    };
    match guard.as_ref() {
        Some(state) => json!({
            "status": if state.last_error.is_some() { "error" } else { "ok" },
            "run_id": state.run.run_id,
            "pid": state.run.pid,
            "paths": state.paths,
            "last_error": state.last_error,
            "in_flight_count": state.in_flight.len(),
        }),
        None => json!({
            "status": "not_configured",
            "detail": "daemon lifecycle ledger not configured in this process",
        }),
    }
}

pub(crate) fn current_paths() -> Option<DaemonLifecyclePaths> {
    let slot = state_slot();
    let guard = slot.lock().ok()?;
    guard.as_ref().map(|state| state.paths.clone())
}

pub(crate) fn current_run_id() -> Option<String> {
    let slot = state_slot();
    let guard = slot.lock().ok()?;
    guard.as_ref().map(|state| state.run.run_id.clone())
}

pub(crate) fn in_flight_tool_calls_for_session(
    session_id: &str,
) -> anyhow::Result<Vec<InFlightToolCallRead>> {
    let slot = state_slot();
    let guard = slot
        .lock()
        .map_err(|_error| anyhow::anyhow!("daemon lifecycle state lock poisoned"))?;
    let Some(state) = guard.as_ref() else {
        bail!("daemon lifecycle ledger is not configured");
    };
    let now = now_unix_ms();
    Ok(state
        .in_flight
        .values()
        .filter(|event| event.mcp_session_id.as_deref() == Some(session_id))
        .map(|event| InFlightToolCallRead {
            seq: event.seq,
            tool: event.tool.clone(),
            mcp_session_id: event.mcp_session_id.clone(),
            started_at_unix_ms: event.started_at_unix_ms,
            elapsed_ms: now.saturating_sub(event.started_at_unix_ms),
            status: event.status.clone(),
        })
        .collect())
}

pub(crate) fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned())
}

fn finish_tool_call(
    seq: u64,
    status: &'static str,
    error: Option<Value>,
    panic: Option<Value>,
    effective_target: Option<Value>,
) -> anyhow::Result<()> {
    if seq == 0 {
        return Ok(());
    }
    let slot = state_slot();
    let mut guard = slot
        .lock()
        .map_err(|_error| anyhow::anyhow!("daemon lifecycle state lock poisoned"))?;
    let Some(state) = guard.as_mut() else {
        bail!("daemon lifecycle ledger is not configured");
    };
    let Some(mut event) = state.in_flight.remove(&seq) else {
        bail!("daemon lifecycle in-flight tool event {seq} is missing");
    };
    let finished_at_unix_ms = now_unix_ms();
    status.clone_into(&mut event.status);
    event.finished_at_unix_ms = Some(finished_at_unix_ms);
    event.duration_ms = Some(finished_at_unix_ms.saturating_sub(event.started_at_unix_ms));
    event.effective_target = effective_target;
    event.error = error;
    event.panic = panic;
    write_tool_event(state, &event);
    Ok(())
}

fn record_panic(info: &std::panic::PanicHookInfo<'_>) -> anyhow::Result<()> {
    let location = info.location().map(|location| {
        json!({
            "file": location.file(),
            "line": location.line(),
            "column": location.column(),
        })
    });
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    append_diagnostic_event(
        "panic",
        "panic",
        json!({
            "payload": payload,
            "location": location,
            "thread": std::thread::current().name(),
        }),
    )
}

fn append_diagnostic_event(
    event_kind: &'static str,
    cause: &'static str,
    detail: Value,
) -> anyhow::Result<()> {
    let slot = state_slot();
    let guard = slot
        .lock()
        .map_err(|_error| anyhow::anyhow!("daemon lifecycle state lock poisoned"))?;
    let Some(state) = guard.as_ref() else {
        bail!("daemon lifecycle ledger is not configured");
    };
    let event = ExitEvent {
        schema_version: SCHEMA_VERSION,
        run_id: state.run.run_id.clone(),
        pid: state.run.pid,
        event_kind: event_kind.to_owned(),
        cause: cause.to_owned(),
        detail,
        recorded_at_unix_ms: now_unix_ms(),
        run: Some(state.run.clone()),
        last_tool_event: read_optional_json(Path::new(&state.paths.tool_last_path))
            .with_context(|| format!("read last tool event {}", state.paths.tool_last_path))?,
        in_flight_tool_events: state.in_flight.values().cloned().collect(),
        paths: state.paths.clone(),
    };
    append_json_line(Path::new(&state.paths.exit_events_path), &event).with_context(|| {
        format!(
            "append daemon diagnostic event {}",
            state.paths.exit_events_path
        )
    })
}

fn record_exit(event_kind: &'static str, cause: &'static str, detail: Value) -> anyhow::Result<()> {
    let slot = state_slot();
    let mut guard = slot
        .lock()
        .map_err(|_error| anyhow::anyhow!("daemon lifecycle state lock poisoned"))?;
    let Some(state) = guard.as_mut() else {
        bail!("daemon lifecycle ledger is not configured");
    };
    let mut run = state.run.clone();
    run.ended_at_unix_ms = Some(now_unix_ms());
    run.ended_reason = Some(cause.to_owned());
    let event = ExitEvent {
        schema_version: SCHEMA_VERSION,
        run_id: state.run.run_id.clone(),
        pid: state.run.pid,
        event_kind: event_kind.to_owned(),
        cause: cause.to_owned(),
        detail,
        recorded_at_unix_ms: now_unix_ms(),
        run: Some(run.clone()),
        last_tool_event: read_optional_json(Path::new(&state.paths.tool_last_path))
            .with_context(|| format!("read last tool event {}", state.paths.tool_last_path))?,
        in_flight_tool_events: state.in_flight.values().cloned().collect(),
        paths: state.paths.clone(),
    };
    append_json_line(Path::new(&state.paths.exit_events_path), &event)
        .with_context(|| format!("append daemon exit event {}", state.paths.exit_events_path))?;
    write_json_atomic(Path::new(&state.paths.run_current_path), &run).with_context(|| {
        format!(
            "write daemon ended current run {}",
            state.paths.run_current_path
        )
    })?;
    state.run = run;
    Ok(())
}

/// Best-effort ledger write (#172): never fails the caller. A failure is kept
/// in `state.last_error` (surfaced by health) and logged rate-limited.
fn write_tool_event(state: &mut DaemonLifecycleState, event: &ToolEvent) {
    match write_tool_event_inner(&state.paths, event) {
        Ok(()) => {
            state.last_error = None;
            tracing::info!(
                code = "MCP_DAEMON_LIFECYCLE_TOOL_EVENT_RECORDED",
                tool = %event.tool,
                status = %event.status,
                seq = event.seq,
                mcp_session_id = event.mcp_session_id.as_deref().unwrap_or("<none>"),
                "daemon lifecycle tool event recorded"
            );
        }
        Err(error) => {
            state.last_error = Some(format!("{error:#}"));
            report_ledger_failure("write_tool_event", &event.tool, &error);
        }
    }
}

fn write_tool_event_inner(paths: &DaemonLifecyclePaths, event: &ToolEvent) -> anyhow::Result<()> {
    let events_path = Path::new(&paths.tool_events_path);
    let line = encode_json_line(events_path, event)?;
    LedgerRotation::from_env().rotate_if_full(events_path, &line)?;
    append_line(events_path, &line)
        .with_context(|| format!("append daemon tool event {}", paths.tool_events_path))?;
    write_json_atomic(Path::new(&paths.tool_last_path), event)
        .with_context(|| format!("write daemon last tool {}", paths.tool_last_path))
}

fn state_slot() -> &'static Mutex<Option<DaemonLifecycleState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    if let Some(slot) = STATE.get() {
        let mut guard = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = None;
    }
}

fn health_detail_for_state(state: &DaemonLifecycleState) -> String {
    let last_error = state
        .last_error
        .as_deref()
        .map_or_else(|| "none".to_owned(), ToOwned::to_owned);
    format!(
        "run_id={} pid={} run_current_path={} tool_last_path={} tool_events_path={} exit_events_path={} in_flight_count={} last_error={}",
        state.run.run_id,
        state.run.pid,
        state.paths.run_current_path,
        state.paths.tool_last_path,
        state.paths.tool_events_path,
        state.paths.exit_events_path,
        state.in_flight.len(),
        last_error
    )
}

fn read_previous_diagnostic<T: DeserializeOwned>(path: &Path) -> Option<T> {
    match read_optional_json(path) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(
                code = "MCP_DAEMON_PREVIOUS_DIAGNOSTIC_IGNORED",
                path = %path.display(),
                detail = %error,
                "cannot read previous diagnostic record; ignoring it and continuing startup"
            );
            None
        }
    }
}

pub(crate) fn read_optional_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("parse JSON {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temp = path.with_extension("tmp");
    {
        let mut file = File::create(&temp).with_context(|| format!("create {}", temp.display()))?;
        serde_json::to_writer_pretty(&mut file, value)
            .with_context(|| format!("encode JSON {}", temp.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("write newline {}", temp.display()))?;
        file.flush()
            .with_context(|| format!("flush {}", temp.display()))?;
        file.sync_data()
            .with_context(|| format!("sync {}", temp.display()))?;
    }
    fs::rename(&temp, path)
        .with_context(|| format!("rename {} to {}", temp.display(), path.display()))
}

fn append_json_line<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let line = encode_json_line(path, value)?;
    append_line(path, &line)
}

fn encode_json_line<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(value)
        .with_context(|| format!("encode JSON line {}", path.display()))?;
    line.push(b'\n');
    Ok(line)
}

fn append_line(path: &Path, line: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open append {}", path.display()))?;
    file.write_all(line)
        .with_context(|| format!("write JSON line {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flush {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("sync {}", path.display()))
}

pub(crate) fn now_unix_ms() -> u64 {
    duration_millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
