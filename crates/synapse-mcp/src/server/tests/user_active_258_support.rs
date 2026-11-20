//! #258 shared test fixtures (cfg(test), wired in server.rs as
//! `user_active_258_support`): an isolated service with the quiet-window env
//! and lease serialized, per-session injected physical-activity readings, real
//! `act` / `process` facade callers and CF_ACTION_LOG readbacks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};
use synapse_core::{error_codes, types::ForegroundContext};

use crate::server::SynapseService;
use crate::server::background_router::ActParams;
use crate::server::g4_unit_support::{error_code, isolated_service};
use crate::server::g7a_support::audit_rows;
use crate::server::m4_tools::ProcessParams;
use crate::server::user_activity::{
    InjectedUserActivity, InjectedUserActivityGuard, inject_user_activity_for_test,
    injected_user_activity_for_test,
};

pub(crate) const USER_ACTIVE: &str = "USER_ACTIVE";
pub(crate) const USER_ACTIVITY_UNKNOWN: &str = "USER_ACTIVITY_UNKNOWN";
pub(crate) const QUIET_ENV: &str = "SYNAPSE_USER_ACTIVE_QUIET_MS";
pub(crate) const REASON: &str = "g258 foreground act regression";
pub(crate) const OWNER_REASON: &str = "owner asked: click Save in Firefox while he keeps typing";
pub(crate) const HUMAN_HWND: i64 = 0x0002_5800;

/// Sets or clears the quiet-window env value; restores the prior value on drop.
pub(crate) struct QuietEnv {
    saved: Option<std::ffi::OsString>,
}

impl QuietEnv {
    fn set(value: Option<&str>) -> Self {
        let saved = std::env::var_os(QUIET_ENV);
        write_quiet_env(value.map(std::ffi::OsStr::new));
        Self { saved }
    }
}

impl Drop for QuietEnv {
    fn drop(&mut self) {
        write_quiet_env(self.saved.as_deref());
    }
}

fn write_quiet_env(value: Option<&std::ffi::OsStr>) {
    // SAFETY: every caller holds `host_env_serial` (the Fixture drops this env
    // guard before the lock); std documents set_var/remove_var as always safe
    // on Windows.
    unsafe {
        match value {
            Some(value) => std::env::set_var(QUIET_ENV, value),
            None => std::env::remove_var(QUIET_ENV),
        }
    }
}

/// Isolated service plus serial guards. Field order is drop order: the service
/// and the env value go before the lease and env locks are released.
pub(crate) struct Fixture {
    pub(crate) service: SynapseService,
    _root: tempfile::TempDir,
    _env: QuietEnv,
    _lease: crate::test_support::LeaseSerialGuard,
    _host_env: std::sync::MutexGuard<'static, ()>,
}

/// `quiet_ms: None` removes the env value, so the default window applies.
pub(crate) fn fixture(quiet_ms: Option<&str>) -> Fixture {
    let host_env = crate::test_support::host_env_serial();
    let lease = crate::test_support::lease_serial("g258-user-active-gate");
    let env = QuietEnv::set(quiet_ms);
    let (root, service) = isolated_service();
    Fixture {
        service,
        _root: root,
        _env: env,
        _lease: lease,
        _host_env: host_env,
    }
}

/// The window the owner was typing in (values from the 2026-10-15 report).
pub(crate) fn human_window() -> ForegroundContext {
    serde_json::from_value(json!({
        "hwnd": HUMAN_HWND, "pid": 4242, "process_name": "Code.exe",
        "process_path": "C:\\Program Files\\Microsoft VS Code\\Code.exe",
        "window_title": "Synapse - Visual Studio Code",
        "window_bounds": {"x": 0, "y": 0, "w": 1280, "h": 800},
        "monitor_index": 0, "dpi_scale": 1.0,
        "is_fullscreen": false, "is_dwm_composed": true
    }))
    .expect("synthetic human window fixture")
}

/// Injects the session's activity reading and reads it back through the seam.
pub(crate) fn inject(
    session: &str,
    monitoring: bool,
    age_ms: Option<u64>,
    window: Option<ForegroundContext>,
) -> InjectedUserActivityGuard {
    let expected_hwnd = window.as_ref().map(|window| window.hwnd);
    let guard = inject_user_activity_for_test(
        session,
        InjectedUserActivity {
            monitoring,
            last_physical_input_age_ms: age_ms,
            user_window: window,
        },
    );
    let read = injected_user_activity_for_test(session).expect("precondition: injected reading");
    assert_eq!(
        read.monitoring, monitoring,
        "precondition: monitoring readback"
    );
    assert_eq!(
        read.last_physical_input_age_ms, age_ms,
        "precondition: age readback"
    );
    assert_eq!(
        read.user_window.map(|window| window.hwnd),
        expected_hwnd,
        "precondition: user window readback"
    );
    guard
}

/// Monitoring on; the human typed `age_ms` ago in `human_window`.
pub(crate) fn human_typed(session: &str, age_ms: u64) -> InjectedUserActivityGuard {
    inject(session, true, Some(age_ms), Some(human_window()))
}

fn merged(mut base: Value, extra: &Value) -> Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    base
}

/// `act operation=foreground` with a `focus_window` action; `extra` overrides
/// top-level keys. Without a session target it ends `TARGET_NOT_SET`.
pub(crate) fn foreground_focus(extra: Value) -> Value {
    let base = json!({
        "operation": "foreground",
        "reason": REASON,
        "ttl_ms": 5_000,
        "action": {"verb": "focus_window"},
    });
    merged(base, &extra)
}

/// `process operation=launch` of a visible-desktop GUI target. The isolated
/// service has no `--allow-launch` policy, so nothing can ever be spawned.
pub(crate) fn notepad_launch(extra: Value) -> Value {
    merged(
        json!({"operation": "launch", "target": "notepad.exe"}),
        &extra,
    )
}

pub(crate) async fn call_act(
    service: &SynapseService,
    session: &str,
    params: Value,
) -> Result<Value, ErrorData> {
    let params: ActParams = serde_json::from_value(params.clone())
        .unwrap_or_else(|error| panic!("#258: act must accept {params}: {error}"));
    let context = crate::test_support::request_context_for_session(session).await;
    service
        .act(Parameters(params), context)
        .await
        .map(|response| serde_json::to_value(response.0).expect("act response JSON"))
}

pub(crate) async fn call_process(
    service: &SynapseService,
    session: &str,
    params: Value,
) -> Result<Value, ErrorData> {
    let params: ProcessParams = serde_json::from_value(params.clone())
        .unwrap_or_else(|error| panic!("#258: process must accept {params}: {error}"));
    let context = crate::test_support::request_context_for_session(session).await;
    service
        .process(Parameters(params), context)
        .await
        .map(|response| serde_json::to_value(response.0).expect("process response JSON"))
}

/// Runs `call_act` on its own task (the caller keeps going while it blocks).
pub(crate) fn spawn_act(
    service: &SynapseService,
    session: &'static str,
    params: Value,
) -> tokio::task::JoinHandle<Result<Value, ErrorData>> {
    let service = service.clone();
    tokio::spawn(async move { call_act(&service, session, params).await })
}

/// Yields until `session`'s act operation=foreground escalation holds its slot.
pub(crate) async fn wait_for_escalation(session: &str) {
    for _ in 0..1_000 {
        // An admitted probe's shared hold is a temporary: it drops right here.
        if crate::server::act_foreground_slot::admit_session_operation(session, "g258 probe")
            .is_err()
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("precondition: the escalation of {session} never claimed its slot");
}

/// The typed refusal's data; it must say nothing was delivered.
pub(crate) fn refusal(result: Result<Value, ErrorData>, code: &str) -> Value {
    let error = match result {
        Ok(response) => panic!("#258: expected a {code} refusal, the act ran: {response}"),
        Err(error) => error,
    };
    assert_eq!(
        error_code(&error).as_deref(),
        Some(code),
        "#258: expected {code}: {error:?}"
    );
    let data = error.data.unwrap_or(Value::Null);
    assert_eq!(data["delivery_state"], "refused_before_delivery", "{data}");
    data
}

/// A process-facade launch refusal: the cause keeps the gate's data.
pub(crate) fn launch_refusal(result: Result<Value, ErrorData>, code: &str) -> ErrorData {
    let error = match result {
        Ok(response) => panic!("#258: expected a {code} launch refusal, it launched: {response}"),
        Err(error) => error,
    };
    assert_eq!(error_code(&error).as_deref(), Some(code), "{error:?}");
    if code == USER_ACTIVE || code == USER_ACTIVITY_UNKNOWN {
        let data = error.data.clone().unwrap_or(Value::Null);
        assert_eq!(
            data["cause"]["delivery_state"], "refused_before_delivery",
            "{data}"
        );
    }
    error
}

/// `tool:verb` of CF_ACTION_LOG rows only an escalation (lease acquire,
/// break_glass profile set) or a delivery (target_act, a started action) writes.
pub(crate) fn escalation_or_delivery_tools(service: &SynapseService) -> Vec<String> {
    audit_rows(service)
        .iter()
        .filter_map(|row| {
            let tool = row["tool"].as_str()?;
            let escalation = tool == "control_lease_acquire" || row["verb"] == "profile_set";
            let delivery = tool == "target_act" || row["status"] == "started";
            let verb = row["verb"].as_str().unwrap_or_default();
            (escalation || delivery).then(|| format!("{tool}:{verb}"))
        })
        .collect()
}

pub(crate) fn assert_nothing_escalated_or_delivered(service: &SynapseService) {
    let touched = escalation_or_delivery_tools(service);
    assert!(
        touched.is_empty(),
        "#258: the refusal must precede lease, profile and input: {touched:?}"
    );
    assert!(
        !synapse_action::lease::status().held,
        "#258: a refusal must leave the input lease free"
    );
}

/// The gate let the call through: break_glass escalated and unwound, the
/// escalation rows exist, and the claimless focus_window step itself answered
/// `TARGET_NOT_SET` (nothing focused, no input).
pub(crate) fn assert_proceeded(service: &SynapseService, result: Result<Value, ErrorData>) {
    let response = match result {
        Ok(response) => response,
        Err(error) => panic!("#258: the gate must let this act through: {error:?}"),
    };
    let action = &response["action"];
    assert_eq!(action["ok"], false, "{response}");
    assert_eq!(
        action["result"]["error"]["code"],
        error_codes::TARGET_NOT_SET,
        "#258: the action step must have run: {response}"
    );
    let escalation = &response["foreground"];
    assert_eq!(escalation["escalated_to"], "break_glass", "{response}");
    assert_eq!(escalation["acquired_lease"], true, "{response}");
    assert_eq!(escalation["profile_restored"], true, "{response}");
    assert!(
        !synapse_action::lease::status().held,
        "the escalation lease is released"
    );
    let touched = escalation_or_delivery_tools(service);
    for row in [
        "control_lease_acquire:lease_acquire",
        "act_foreground:profile_set",
    ] {
        assert!(
            touched.iter().any(|seen| seen == row),
            "#258: escalation row {row} missing: {touched:?}"
        );
    }
}

/// Command-audit `final` rows of `tool`, oldest first.
pub(crate) fn final_rows(service: &SynapseService, tool: &str) -> Vec<Value> {
    audit_rows(service)
        .into_iter()
        .filter(|row| row["tool"] == tool && row["phase"] == "final")
        .collect()
}

pub(crate) fn final_act_row(service: &SynapseService) -> Value {
    let mut finals = final_rows(service, "act");
    assert_eq!(finals.len(), 1, "one final act audit row: {finals:?}");
    finals.pop().expect("final act row")
}
