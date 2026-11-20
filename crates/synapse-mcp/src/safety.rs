use std::time::{Duration, Instant};

use serde::Serialize;
use synapse_action::{
    ActionError, ActionResult, OperatorHotkeyGuard, OperatorHotkeyStatus, RELEASE_ALL_HANDLE,
    ReleaseAllCompletion, ReleaseAllOutcome, set_operator_hotkey_status,
};
use synapse_core::error_codes;
use tokio::runtime::Handle;

use crate::m3::SharedM3State;
use crate::server::SynapseService;

pub const DISABLE_OPERATOR_HOTKEY_ENV: &str = "SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY";
/// When set truthy, a failure to register the operator panic hotkey aborts
/// startup instead of degrading. Off by default so a leaked/duplicate instance
/// holding the global hotkey cannot brick the MCP server (the failure surfaced
/// as JSON-RPC `-32000` to clients and broke the editor-wired stdio child).
pub const REQUIRE_OPERATOR_HOTKEY_ENV: &str = "SYNAPSE_MCP_REQUIRE_OPERATOR_HOTKEY";

/// Operator-facing remediation for an unavailable panic hotkey.
const OPERATOR_HOTKEY_REMEDIATION: &str = "the daemon-owned operator hotkey could not be armed; stop duplicate synapse-mcp instances or conflicting hook owners, set SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY=1 to run intentionally without it, set SYNAPSE_MCP_REQUIRE_OPERATOR_HOTKEY=1 to make this a hard startup failure, or set SYNAPSE_OPERATOR_HOTKEY / SYNAPSE_MCP_OPERATOR_HOTKEY to another Ctrl+Alt+Shift+<A-Z|0-9> chord";
/// Overrides how long the operator hotkey waits for the PRIMARY `release_all`
/// acknowledgement, in milliseconds (clamped to `MAX_OPERATOR_RELEASE_ALL_WAIT_MS`).
/// It is only a timeout: `within_budget` is always judged against the documented
/// `K1_RELEASE_ALL_TARGET` (#95).
pub const OPERATOR_RELEASE_ALL_BUDGET_ENV: &str = "SYNAPSE_OPERATOR_RELEASE_ALL_BUDGET_MS";
/// Overrides how long the operator hotkey additionally waits for the combined
/// `release_all` result (the `ViGEm`/hardware secondary drains), in milliseconds.
pub const OPERATOR_RELEASE_ALL_COMPLETION_ENV: &str = "SYNAPSE_OPERATOR_RELEASE_ALL_COMPLETION_MS";
const DEFAULT_OPERATOR_RELEASE_ALL_BUDGET_MS: u64 = 50;
const DEFAULT_OPERATOR_RELEASE_ALL_COMPLETION_MS: u64 = 250;
/// Upper bound for both waits so a misconfigured value cannot block the hotkey
/// thread (and the K2 fleet kill queued behind it) for long.
const MAX_OPERATOR_RELEASE_ALL_WAIT_MS: u64 = 1000;
/// Documented K1 latency target the `within_budget` report is judged against.
pub(crate) const K1_RELEASE_ALL_TARGET: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DisableReport {
    pub(crate) result: &'static str,
    pub(crate) disabled_ids: Vec<String>,
    pub(crate) error_code: Option<&'static str>,
    pub(crate) detail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseAllReport {
    pub(crate) result: &'static str,
    pub(crate) error_code: Option<&'static str>,
    pub(crate) detail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorHotkeyImmediateReport {
    pub hotkey: &'static str,
    pub lease_before: synapse_action::LeaseStatus,
    pub preempted_lease: Option<synapse_action::LeaseStatus>,
    pub lease_after_preempt: synapse_action::LeaseStatus,
    pub disable_report: DisableReport,
    pub release_all_report: ReleaseAllReport,
    pub elapsed_ms: u128,
    pub within_budget: bool,
}

pub fn install_operator_hotkey(
    service: SynapseService,
) -> synapse_action::ActionResult<Option<OperatorHotkeyGuard>> {
    if operator_hotkey_disabled_by_env()? {
        tracing::warn!(
            code = "SAFETY_OPERATOR_HOTKEY_DISABLED",
            env = DISABLE_OPERATOR_HOTKEY_ENV,
            "operator hotkey disabled by explicit environment override"
        );
        set_operator_hotkey_status(OperatorHotkeyStatus::DisabledByEnv);
        return Ok(None);
    }
    let release_all_budget = operator_release_all_budget()?;
    let release_all_completion_wait = operator_release_all_completion_wait()?;
    let runtime = Handle::current();
    match synapse_action::install_operator_hotkey(move || {
        handle_operator_hotkey(
            &service,
            &runtime,
            release_all_budget,
            release_all_completion_wait,
        );
    }) {
        Ok(guard) => {
            set_operator_hotkey_status(OperatorHotkeyStatus::Registered);
            Ok(Some(guard))
        }
        Err(error) => {
            set_operator_hotkey_status(OperatorHotkeyStatus::Unavailable);
            if operator_hotkey_required_by_env()? {
                // Strict mode: caller propagates and startup fails closed.
                return Err(error);
            }
            // Default: do NOT abort the whole MCP server because the global
            // kill-switch could not bind. Log loudly with exact cause and
            // remediation, record degraded status for /health, and continue so
            // the (mostly read-only) tool surface stays usable. Input-emitting
            // tools remain guarded by their own preflight/consent paths.
            tracing::error!(
                code = error_codes::ACTION_BACKEND_UNAVAILABLE,
                component = "operator_hotkey",
                hotkey = synapse_action::hotkey::configured_operator_hotkey_label(),
                status = OperatorHotkeyStatus::Unavailable.label(),
                error = %error,
                remediation = OPERATOR_HOTKEY_REMEDIATION,
                require_env = REQUIRE_OPERATOR_HOTKEY_ENV,
                disable_env = DISABLE_OPERATOR_HOTKEY_ENV,
                "operator panic hotkey unavailable; continuing in degraded safety mode without the kill-switch"
            );
            Ok(None)
        }
    }
}

fn operator_hotkey_required_by_env() -> synapse_action::ActionResult<bool> {
    parse_bool_env(REQUIRE_OPERATOR_HOTKEY_ENV)
}

fn operator_hotkey_disabled_by_env() -> synapse_action::ActionResult<bool> {
    parse_bool_env(DISABLE_OPERATOR_HOTKEY_ENV)
}

fn operator_release_all_budget() -> ActionResult<Duration> {
    parse_release_all_budget(env_text(OPERATOR_RELEASE_ALL_BUDGET_ENV).as_deref())
}

fn operator_release_all_completion_wait() -> ActionResult<Duration> {
    parse_release_all_completion_wait(env_text(OPERATOR_RELEASE_ALL_COMPLETION_ENV).as_deref())
}

fn env_text(name: &str) -> Option<String> {
    std::env::var_os(name).map(|raw| raw.to_string_lossy().into_owned())
}

fn parse_release_all_budget(raw: Option<&str>) -> ActionResult<Duration> {
    parse_clamped_wait_ms(
        OPERATOR_RELEASE_ALL_BUDGET_ENV,
        raw,
        DEFAULT_OPERATOR_RELEASE_ALL_BUDGET_MS,
    )
}

fn parse_release_all_completion_wait(raw: Option<&str>) -> ActionResult<Duration> {
    parse_clamped_wait_ms(
        OPERATOR_RELEASE_ALL_COMPLETION_ENV,
        raw,
        DEFAULT_OPERATOR_RELEASE_ALL_COMPLETION_MS,
    )
}

/// Parses a positive millisecond count (blank/unset -> `default_ms`) and clamps
/// it to `MAX_OPERATOR_RELEASE_ALL_WAIT_MS`.
fn parse_clamped_wait_ms(name: &str, raw: Option<&str>, default_ms: u64) -> ActionResult<Duration> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Duration::from_millis(default_ms));
    };
    match raw.parse::<u64>() {
        Ok(ms) if ms > 0 => Ok(Duration::from_millis(
            ms.min(MAX_OPERATOR_RELEASE_ALL_WAIT_MS),
        )),
        _ => Err(ActionError::BackendUnavailable {
            detail: format!("{name} must be a positive integer number of milliseconds"),
        }),
    }
}

/// True when the PRIMARY release finished inside the documented K1 target,
/// independent of the (possibly larger) configured timeout.
pub(crate) fn within_k1_target(elapsed: Duration) -> bool {
    elapsed <= K1_RELEASE_ALL_TARGET
}

fn parse_bool_env(name: &str) -> synapse_action::ActionResult<bool> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(false);
    };
    let value = raw.to_string_lossy();
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "" | "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ActionError::BackendUnavailable {
            detail: format!("{name} must be one of 1/true/yes/on or 0/false/no/off"),
        }),
    }
}

fn handle_operator_hotkey(
    service: &SynapseService,
    runtime: &Handle,
    budget: Duration,
    completion_wait: Duration,
) {
    let started = Instant::now();
    let emergency_reservation = synapse_action::lease::reserve_emergency_cleanup();
    let lease_before = synapse_action::lease::status();
    let preempted_lease = synapse_action::force_preempt_input_lease("operator_hotkey");
    let disable_report = DisableReport {
        result: "deferred",
        disabled_ids: Vec::new(),
        error_code: None,
        detail: Some(
            "reflex disable deferred to operator hotkey K2 after immediate input release"
                .to_owned(),
        ),
    };
    let release_started = Instant::now();
    let release_all_fire = fire_release_all(budget, completion_wait);
    let release_all_report = release_all_fire.report;
    let release_all_elapsed = release_started.elapsed();
    let elapsed = started.elapsed();
    // Judge the K1 target on hotkey entry -> PRIMARY release acknowledged; the
    // bounded wait for secondary drains is reported separately.
    let primary_elapsed = release_all_fire
        .primary_ack_after
        .map(|ack_after| release_started.duration_since(started) + ack_after);
    let lease_after_preempt = synapse_action::lease::status();
    let immediate = OperatorHotkeyImmediateReport {
        hotkey: synapse_action::hotkey::configured_operator_hotkey_label(),
        lease_before,
        preempted_lease: preempted_lease.clone(),
        lease_after_preempt,
        disable_report: disable_report.clone(),
        release_all_report: release_all_report.clone(),
        elapsed_ms: elapsed.as_millis(),
        within_budget: primary_elapsed.is_some_and(within_k1_target),
    };
    tracing::warn!(
        code = error_codes::SAFETY_OPERATOR_HOTKEY_FIRED,
        hotkey = immediate.hotkey,
        input_lease_preempted = preempted_lease.is_some(),
        input_lease_prior_owner = ?preempted_lease
            .as_ref()
            .and_then(|status| status.owner_session_id.clone()),
        input_lease_operator_owner = synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID,
        input_lease_operator_ttl_ms = synapse_action::OPERATOR_PREEMPT_LEASE_TTL_MS,
        disabled_reflexes = disable_report.disabled_ids.len(),
        disabled_reflex_ids = ?disable_report.disabled_ids,
        reflex_result = disable_report.result,
        reflex_error_code = ?disable_report.error_code,
        reflex_detail = ?disable_report.detail,
        release_all_result = release_all_report.result,
        release_all_error_code = ?release_all_report.error_code,
        release_all_detail = ?release_all_report.detail,
        elapsed_ms = immediate.elapsed_ms,
        release_all_ack_wait_ms = release_all_elapsed.as_millis(),
        primary_ack_ms = ?primary_elapsed.map(|elapsed| elapsed.as_millis()),
        configured_timeout_ms = budget.as_millis(),
        completion_wait_ms = completion_wait.as_millis(),
        target_ms = K1_RELEASE_ALL_TARGET.as_millis(),
        within_budget = immediate.within_budget,
        "operator hotkey released input and queued reflex shutdown plus K2 fleet kill"
    );
    let service = service.clone();
    let _operator_panic_task = runtime.spawn(async move {
        let _emergency_reservation = emergency_reservation;
        if let Err(error) = service.operator_panic_kill_all(immediate).await {
            tracing::error!(
                code = error_codes::SAFETY_OPERATOR_HOTKEY_FIRED,
                error_code = error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("code"))
                    .and_then(serde_json::Value::as_str),
                detail = %error.message,
                "operator hotkey K2 fleet kill task failed"
            );
        }
    });
}

pub(crate) fn disable_reflexes(m3_state: &SharedM3State) -> DisableReport {
    let runtime = match m3_state.lock() {
        Ok(state) => state.reflex_runtime.clone(),
        Err(_err) => {
            return DisableReport {
                result: "error",
                disabled_ids: Vec::new(),
                error_code: Some(error_codes::TOOL_INTERNAL_ERROR),
                detail: Some("M3 service state lock poisoned".to_owned()),
            };
        }
    };
    let Some(runtime) = runtime else {
        return DisableReport {
            result: "not_initialized",
            disabled_ids: Vec::new(),
            error_code: None,
            detail: None,
        };
    };
    let mut runtime = match runtime.lock() {
        Ok(runtime) => runtime,
        Err(_err) => {
            return DisableReport {
                result: "error",
                disabled_ids: Vec::new(),
                error_code: Some(error_codes::TOOL_INTERNAL_ERROR),
                detail: Some("reflex runtime lock poisoned".to_owned()),
            };
        }
    };
    match runtime.disable_all_by_operator() {
        Ok(disabled) => DisableReport {
            result: "ok",
            disabled_ids: disabled.into_iter().map(|status| status.id).collect(),
            error_code: None,
            detail: None,
        },
        Err(error) => DisableReport {
            result: "error",
            disabled_ids: Vec::new(),
            error_code: Some(error.code()),
            detail: Some(error.to_string()),
        },
    }
}

/// K1 `release_all` report plus how long the PRIMARY release took to acknowledge
/// (`None` when no acknowledgement arrived).
pub(crate) struct ReleaseAllFire {
    pub(crate) report: ReleaseAllReport,
    pub(crate) primary_ack_after: Option<Duration>,
}

fn fire_release_all(budget: Duration, completion_wait: Duration) -> ReleaseAllFire {
    let Some(handle) = RELEASE_ALL_HANDLE.get() else {
        return ReleaseAllFire {
            report: ReleaseAllReport {
                result: "missing_handle",
                error_code: Some(error_codes::ACTION_BACKEND_UNAVAILABLE),
                detail: Some("RELEASE_ALL_HANDLE is not initialized".to_owned()),
            },
            primary_ack_after: None,
        };
    };
    release_all_fire_from_outcome(handle.fire_release_all_blocking(budget, completion_wait))
}

/// Maps the emitter outcome to the K1 report so a failed or still-running
/// secondary drain is never reported as a plain `ok`.
pub(crate) fn release_all_fire_from_outcome(
    outcome: ActionResult<ReleaseAllOutcome>,
) -> ReleaseAllFire {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            return ReleaseAllFire {
                report: error_report(&error),
                primary_ack_after: None,
            };
        }
    };
    let report = match (&outcome.primary, &outcome.completion) {
        (Err(error), _) => error_report(error),
        (Ok(()), ReleaseAllCompletion::Complete(Ok(()))) => ReleaseAllReport {
            result: "ok",
            error_code: None,
            detail: None,
        },
        (Ok(()), ReleaseAllCompletion::Complete(Err(error))) => ReleaseAllReport {
            result: "partial_failure",
            error_code: Some(error.code()),
            detail: Some(format!(
                "primary backend released; secondary drain failed: {error}"
            )),
        },
        (Ok(()), ReleaseAllCompletion::Pending) => ReleaseAllReport {
            result: "ok_secondary_pending",
            error_code: None,
            detail: Some(
                "primary backend released; secondary drains were still running when the bounded wait ended"
                    .to_owned(),
            ),
        },
    };
    ReleaseAllFire {
        report,
        primary_ack_after: Some(outcome.primary_ack_after),
    }
}

fn error_report(error: &ActionError) -> ReleaseAllReport {
    ReleaseAllReport {
        result: "error",
        error_code: Some(error.code()),
        detail: Some(error.to_string()),
    }
}

#[cfg(test)]
#[path = "tests/safety_release_all_budget_95.rs"]
mod safety_release_all_budget_95;
