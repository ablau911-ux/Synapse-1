//! Read-only identity of the window most recently used by a person (#106).

use std::time::{SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use synapse_core::{error_codes, types::ForegroundContext};

use super::user_active_gate::owner_presence::{
    OwnerPresence, PresenceInput, last_owner_return_ns, owner_presence,
};
use super::user_active_gate::quiet_window_ms;
use super::{ErrorData, Json, Parameters, SynapseService, mcp_error, tool, tool_router};
use crate::m3::activity_recorder::LiveInputReadback;

const SOURCE_OF_TRUTH: &str = "physical Windows keyboard/mouse hook event (user_window re-read by HWND) + live GetForegroundWindow readback (current_foreground); dpi_scale per window from GetDpiForWindow";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserActivityOperation {
    #[default]
    Status,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserActivityParams {
    #[serde(default)]
    pub operation: UserActivityOperation,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserActivityResponse {
    pub operation: UserActivityOperation,
    pub source_of_truth: &'static str,
    pub captured_at_unix_ms: u64,
    pub physical_input_monitoring: bool,
    /// Last window receiving physical input; absent until a person uses the PC.
    pub user_window: Option<ForegroundContext>,
    pub last_physical_input_age_ms: Option<u64>,
    pub last_physical_input_kind: Option<&'static str>,
    /// The present foreground may differ from the last human-used window.
    pub current_foreground: Option<ForegroundContext>,
    pub foreground_read_error: Option<String>,
    /// Owner-away mode verdict the real-input gate would use now: `away`,
    /// `active` (with `retry_after_ms`), `locked` or `unknown`.
    pub owner_presence: OwnerPresence,
    /// Age of the last owner-return stop (the owner's physical input arrived
    /// while an agent held the foreground input lease); absent if none.
    pub last_owner_return_stop_age_ms: Option<u64>,
}

pub(super) fn age_ms(now_ns: u64, timestamp_ns: u64) -> u64 {
    now_ns.saturating_sub(timestamp_ns) / 1_000_000
}

/// Wall-clock nanoseconds since the Unix epoch (the hook marker's clock).
pub(super) fn unix_now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// The last physical-input window re-read by HWND; `None` when it is gone or
/// the HWND now belongs to another process.
pub(super) fn current_user_window(live: &LiveInputReadback) -> Option<ForegroundContext> {
    let recorded = live.user_window.as_ref()?;
    synapse_a11y::foreground_context(recorded.hwnd)
        .ok()
        .filter(|current| current.pid == recorded.pid)
}

#[tool_router(router = user_activity_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Identify the window most recently used with a physical keyboard or mouse. Also report the current foreground window so an agent can distinguish human work from its own injected actions. Read-only; the agent decides how to proceed."
    )]
    pub async fn user_activity(
        &self,
        params: Parameters<UserActivityParams>,
    ) -> Result<Json<UserActivityResponse>, ErrorData> {
        let now_ns = unix_now_ns();
        let recorder = self
            .m3_state
            .lock()
            .map_err(|_| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "activity state lock poisoned",
                )
            })?
            .activity_recorder
            .clone();
        let live = recorder
            .as_ref()
            .map(|recorder| recorder.live_input_readback());
        let foreground_read = synapse_a11y::current_foreground_context();
        let (current_foreground, foreground_read_error) = match foreground_read {
            Ok(foreground) => (Some(foreground), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let user_window = live.as_ref().and_then(current_user_window);
        let last_physical_input_age_ms = live
            .as_ref()
            .and_then(|live| live.last_physical_ns.map(|ns| age_ms(now_ns, ns)));
        let presence = owner_presence(
            PresenceInput {
                monitoring: live.as_ref().is_some_and(|live| live.monitoring),
                locked: synapse_a11y::workstation_locked(),
                last_physical_age_ms: last_physical_input_age_ms,
                monitor_age_ms: live
                    .as_ref()
                    .and_then(|live| live.monitor_started_ns.map(|ns| age_ms(now_ns, ns))),
            },
            quiet_window_ms(),
        );
        Ok(Json(UserActivityResponse {
            operation: params.0.operation,
            source_of_truth: SOURCE_OF_TRUTH,
            captured_at_unix_ms: now_ns / 1_000_000,
            physical_input_monitoring: live.as_ref().is_some_and(|live| live.monitoring),
            user_window,
            last_physical_input_age_ms,
            last_physical_input_kind: live.as_ref().and_then(|live| live.last_physical_kind),
            current_foreground,
            foreground_read_error,
            owner_presence: presence,
            last_owner_return_stop_age_ms: last_owner_return_ns().map(|ns| age_ms(now_ns, ns)),
        }))
    }
}

/// #258 test seam: a resolved physical user-activity reading injected per MCP
/// session id. The foreground user-activity gate (`user_active_gate`) uses it
/// (under `cfg(test)`) for that session in place of the live activity recorder
/// and the HWND re-read; sessions without an injected reading use the real
/// source.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct InjectedUserActivity {
    pub(crate) monitoring: bool,
    pub(crate) last_physical_input_age_ms: Option<u64>,
    pub(crate) user_window: Option<ForegroundContext>,
}

#[cfg(test)]
type InjectedUserActivityMap = std::collections::BTreeMap<String, InjectedUserActivity>;

#[cfg(test)]
fn injected_user_activity_map() -> &'static std::sync::Mutex<InjectedUserActivityMap> {
    static INJECTED: std::sync::OnceLock<std::sync::Mutex<InjectedUserActivityMap>> =
        std::sync::OnceLock::new();
    INJECTED.get_or_init(Default::default)
}

/// #258 test seam: removes the session's injected reading on drop.
#[cfg(test)]
pub(crate) struct InjectedUserActivityGuard {
    session_id: String,
}

#[cfg(test)]
impl Drop for InjectedUserActivityGuard {
    fn drop(&mut self) {
        injected_user_activity_map()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.session_id);
    }
}

/// #258 test seam: inject `reading` for `session_id` until the guard drops.
#[cfg(test)]
pub(crate) fn inject_user_activity_for_test(
    session_id: &str,
    reading: InjectedUserActivity,
) -> InjectedUserActivityGuard {
    injected_user_activity_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(session_id.to_owned(), reading);
    InjectedUserActivityGuard {
        session_id: session_id.to_owned(),
    }
}

/// #258 test seam: the reading injected for `session_id`, if any.
#[cfg(test)]
pub(crate) fn injected_user_activity_for_test(session_id: &str) -> Option<InjectedUserActivity> {
    injected_user_activity_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session_id)
        .cloned()
}

#[cfg(test)]
#[path = "tests/user_activity_223.rs"]
mod user_activity_223_tests;
