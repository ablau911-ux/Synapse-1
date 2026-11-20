//! #258: user-workspace awareness on the action side.
//!
//! Before Synapse moves the human's real foreground, cursor or keyboard focus
//! (`act operation=foreground`, focus_window, the SendInput tiers of
//! click/type/press/scroll/stroke/keymap, visible-desktop GUI launches) it
//! reads the physical user-activity source that `user_activity` reports.
//! Physical input younger than the quiet window refuses with `USER_ACTIVE`; an
//! input hook that is not running refuses with `USER_ACTIVITY_UNKNOWN` (fail
//! closed). Owner-away mode adds: a locked workstation refuses with
//! `USER_WORKSTATION_LOCKED` (also for an owner-approved call), no physical
//! input since a monitor younger than the quiet window counts as active, a
//! physical mouse move counts as input, and every refusal names its
//! `owner_presence` and when to retry (`retry_after_ms`, `retry_when`). All
//! refusals happen before any lease, profile change, focus change or input
//! and carry `delivery_state=refused_before_delivery`. The stop when the
//! owner returns mid-action lives in `owner_presence::observe_input_event`.
//!
//! Background target-scoped delivery (PostMessage/UIA/CDP to a claimed
//! window) and passive perception never call this gate. Synapse's own
//! injected input is not physical input: the hook marker records only
//! non-injected events, so it never trips the gate.
//!
//! The owner override is per call (`owner_override`): a launch carries its
//! own `owner_approved` + reason; an approved `act operation=foreground`
//! passes its inner gates only inside its own escalation task.

use std::sync::Arc;

use rmcp::model::ErrorCode;
use serde_json::json;
use synapse_core::{error_codes, types::ForegroundContext};

// Owner-away decisions (Phase 5). Declared here because this gate is their
// one owner; `server.rs` is outside this lane.
#[path = "owner_presence.rs"]
pub(crate) mod owner_presence;

use self::owner_presence::{
    OwnerPresence, PresenceInput, admission, owner_presence, quiet_window_from,
};
use super::owner_override::{ApprovedCall, OwnerOverride, in_approved_call};
use super::{ErrorData, SynapseService, mcp_error};
use crate::m3::activity_recorder::ActivityRecorder;

/// Quiet-window config in milliseconds. Unset or unparsable keeps the
/// default (the gate stays on); `0` disables the gate.
pub(crate) const QUIET_WINDOW_ENV: &str = "SYNAPSE_USER_ACTIVE_QUIET_MS";
/// Route name of the `act operation=foreground` entry gate.
const FOREGROUND_ESCALATION_ROUTE: &str = "act operation=foreground";
/// Route name of the visible GUI launch gate.
const LAUNCH_ROUTE: &str = "act_launch";
const SOURCE_OF_TRUTH: &str = "physical Windows keyboard/mouse hook marker (user_activity; user_window re-read by HWND) + SYNAPSE_USER_ACTIVE_QUIET_MS quiet window + OpenInputDesktop/lock-surface workstation lock probe";
const FOREGROUND_RECOVERY: &str = "wait until the human has been idle for the quiet window, use a background target-scoped route (act operation=invoke on a claimed window), or, only when the owner asked for this exact action, call act operation=foreground with owner_approved=true and a non-empty reason";
const LAUNCH_RECOVERY: &str = "wait until the human has been idle for the quiet window, launch on the session's agent desktop (desktop=agent:session), or, only when the owner asked for this exact launch, repeat process operation=launch with owner_approved=true and a non-empty reason";

/// One resolved physical-activity reading.
#[derive(Clone, Debug)]
struct UserActivityReading {
    monitoring: bool,
    last_physical_input_age_ms: Option<u64>,
    user_window: Option<ForegroundContext>,
    /// Owner-away mode: workstation locked / secure desktop owns input.
    locked: bool,
    /// How long the input monitor has been running.
    monitor_age_ms: Option<u64>,
}

impl UserActivityReading {
    fn unavailable() -> Self {
        Self {
            monitoring: false,
            last_physical_input_age_ms: None,
            user_window: None,
            locked: synapse_a11y::workstation_locked(),
            monitor_age_ms: None,
        }
    }

    const fn presence_input(&self) -> PresenceInput {
        PresenceInput {
            monitoring: self.monitoring,
            locked: self.locked,
            last_physical_age_ms: self.last_physical_input_age_ms,
            monitor_age_ms: self.monitor_age_ms,
        }
    }
}

/// The gate for one route of one session: the activity source plus who asks.
#[derive(Clone, Debug)]
pub(crate) struct UserActiveGate {
    route: &'static str,
    /// Route-specific way out, named in the refusal.
    recovery: &'static str,
    session_id: Option<String>,
    recorder: Option<Arc<ActivityRecorder>>,
}

impl UserActiveGate {
    /// `Ok` when `route` may move the human's foreground, cursor or keyboard
    /// focus now; otherwise the typed refusal (nothing was delivered). Inside
    /// an owner-approved call's own task the gate passes for that call only.
    pub(crate) fn check(&self) -> Result<(), ErrorData> {
        self.check_as(in_approved_call(self.session_id.as_deref()))
    }

    /// The gate with an explicit owner approval: an approved call passes an
    /// active or unknown owner, never a locked workstation.
    fn check_as(&self, approved: bool) -> Result<(), ErrorData> {
        self.decide(&self.reading(), quiet_window_ms(), approved)
    }

    fn reading(&self) -> UserActivityReading {
        if let Some(injected) = injected_reading(self.session_id.as_deref()) {
            return injected;
        }
        let Some(recorder) = self.recorder.as_ref() else {
            return UserActivityReading::unavailable();
        };
        let live = recorder.live_input_readback();
        let now_ns = super::user_activity::unix_now_ns();
        let current_window = super::user_activity::current_user_window(&live);
        UserActivityReading {
            monitoring: live.monitoring,
            last_physical_input_age_ms: live
                .last_physical_ns
                .map(|ns| super::user_activity::age_ms(now_ns, ns)),
            // A closed/reused HWND still names the window the human last used.
            user_window: current_window.or(live.user_window),
            locked: synapse_a11y::workstation_locked(),
            monitor_age_ms: live
                .monitor_started_ns
                .map(|ns| super::user_activity::age_ms(now_ns, ns)),
        }
    }
}

/// #258 test seam: the reading injected for this session replaces the live
/// recorder and the HWND re-read.
#[cfg(test)]
fn injected_reading(session_id: Option<&str>) -> Option<UserActivityReading> {
    let injected = super::user_activity::injected_user_activity_for_test(session_id?)?;
    Some(UserActivityReading {
        monitoring: injected.monitoring,
        last_physical_input_age_ms: injected.last_physical_input_age_ms,
        user_window: injected.user_window,
        // An injected reading stands for a long-running, unlocked monitor, so
        // the #258 rows keep their meaning (no input since start = away).
        locked: false,
        monitor_age_ms: Some(u64::MAX),
    })
}

#[cfg(not(test))]
const fn injected_reading(_session_id: Option<&str>) -> Option<UserActivityReading> {
    None
}

/// Quiet window from [`QUIET_WINDOW_ENV`], default 60000 ms.
pub(super) fn quiet_window_ms() -> u64 {
    quiet_window_from(std::env::var(QUIET_WINDOW_ENV).ok().as_deref())
}

impl UserActiveGate {
    /// The decision for one reading ([`owner_presence`] + [`admission`]).
    fn decide(
        &self,
        reading: &UserActivityReading,
        quiet_ms: u64,
        approved: bool,
    ) -> Result<(), ErrorData> {
        let presence = owner_presence(reading.presence_input(), quiet_ms);
        let Err(code) = admission(presence, approved) else {
            return Ok(());
        };
        let route = self.route;
        let message = match (presence, reading.last_physical_input_age_ms) {
            (OwnerPresence::Locked, _) => format!(
                "{route} refused: the workstation is locked (or a secure desktop owns input), so no real keyboard/mouse input is delivered"
            ),
            (OwnerPresence::Active { .. }, Some(age_ms)) => format!(
                "{route} refused: the human used the keyboard/mouse {age_ms} ms ago{} (quiet window {quiet_ms} ms)",
                window_label(reading.user_window.as_ref())
            ),
            (OwnerPresence::Active { .. }, None) => format!(
                "{route} refused: physical input monitoring started less than the quiet window ({quiet_ms} ms) ago, so Synapse cannot yet tell that the human is away"
            ),
            _ if reading.monitoring => format!(
                "{route} refused: the input monitor start time is unknown, so Synapse cannot tell whether the human is using the PC"
            ),
            _ => format!(
                "{route} refused: physical keyboard/mouse monitoring is not running, so Synapse cannot tell whether the human is using the PC"
            ),
        };
        Err(self.refusal(code, presence, reading, quiet_ms, &message))
    }

    fn refusal(
        &self,
        code: &'static str,
        presence: OwnerPresence,
        reading: &UserActivityReading,
        quiet_ms: u64,
        message: &str,
    ) -> ErrorData {
        let recovery = self.recovery;
        let (retry_after_ms, retry_when) = match presence {
            OwnerPresence::Active { retry_after_ms } => (
                Some(retry_after_ms),
                format!("retry in {retry_after_ms} ms if the human stays idle"),
            ),
            OwnerPresence::Locked => (
                None,
                format!(
                    "retry after the workstation is unlocked and the human has then been idle for {quiet_ms} ms"
                ),
            ),
            OwnerPresence::Unknown | OwnerPresence::Away => (
                None,
                "retry once physical keyboard/mouse monitoring is running".to_owned(),
            ),
        };
        ErrorData::new(
            ErrorCode(-32099),
            format!("{message}; nothing was delivered; {retry_when}. {recovery}"),
            Some(json!({
                "code": code,
                "owner_presence": presence,
                "retry_after_ms": retry_after_ms,
                "retry_when": retry_when,
                "workstation_locked": reading.locked,
                "delivery_state": "refused_before_delivery",
                "route": self.route,
                "user_window": reading.user_window,
                "last_physical_input_age_ms": reading.last_physical_input_age_ms,
                "physical_input_monitoring": reading.monitoring,
                "quiet_window_ms": quiet_ms,
                "quiet_window_config": QUIET_WINDOW_ENV,
                "source_of_truth": SOURCE_OF_TRUTH,
                "recovery": recovery,
            })),
        )
    }
}

fn window_label(window: Option<&ForegroundContext>) -> String {
    window.map_or_else(String::new, |window| {
        format!(
            " in {} \"{}\" (hwnd {})",
            window.process_name, window.window_title, window.hwnd
        )
    })
}

impl SynapseService {
    /// The #258 gate for `route` on behalf of `session_id` (`None` = stdio).
    pub(crate) fn user_active_gate(
        &self,
        route: &'static str,
        session_id: Option<&str>,
    ) -> Result<UserActiveGate, ErrorData> {
        self.user_active_gate_with(route, FOREGROUND_RECOVERY, session_id)
    }

    fn user_active_gate_with(
        &self,
        route: &'static str,
        recovery: &'static str,
        session_id: Option<&str>,
    ) -> Result<UserActiveGate, ErrorData> {
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
        Ok(UserActiveGate {
            route,
            recovery,
            session_id: session_id.map(ToOwned::to_owned),
            recorder,
        })
    }

    /// Refuses `route` with `USER_ACTIVE` / `USER_ACTIVITY_UNKNOWN` before any
    /// lease, focus change or input while the human is (or may be) active.
    pub(crate) fn ensure_user_idle(
        &self,
        route: &'static str,
        session_id: Option<&str>,
    ) -> Result<(), ErrorData> {
        self.user_active_gate(route, session_id)?.check()
    }

    /// Launch gate: a GUI process on a visible desktop can raise a window over
    /// the human's work, so it is gated unless this launch carries its own
    /// owner override. Hidden console helpers and launches on a session-owned
    /// agent desktop (`desktop=agent:...`) are not gated.
    pub(crate) fn ensure_user_idle_for_launch(
        &self,
        params: &crate::m4::ActLaunchParams,
        session_id: Option<&str>,
        owner_override: Option<&OwnerOverride>,
    ) -> Result<(), ErrorData> {
        let agent_desktop = params
            .desktop
            .as_deref()
            .is_some_and(|desktop| desktop.starts_with("agent:"));
        if agent_desktop || crate::m4::launch_target_needs_new_console(&params.target) {
            return Ok(());
        }
        if owner_override.is_some() {
            tracing::info!(
                code = "USER_ACTIVE_OWNER_APPROVED",
                route = LAUNCH_ROUTE,
                session_id = session_id.unwrap_or("stdio"),
                "launch bypasses the user-activity gate: owner_approved=true with a reason"
            );
            return Ok(());
        }
        self.user_active_gate_with(LAUNCH_ROUTE, LAUNCH_RECOVERY, session_id)?
            .check()
    }

    /// Shell classification uses the same activity decision and task-local override.
    pub(crate) fn ensure_user_idle_for_shell_launch(
        &self,
        session_id: &str,
    ) -> Result<(), ErrorData> {
        self.user_active_gate_with("shell GUI launch", LAUNCH_RECOVERY, Some(session_id))?
            .check()
    }

    /// Entry gate of `act operation=foreground`, run after the reason is
    /// validated and before the slot, lease or profile change. With
    /// `owner_approved` (the caller already proved a non-empty reason) the
    /// returned approval, scoped to this call's escalation task, also lets
    /// that call's own focus and SendInput steps through; otherwise the human
    /// must be idle.
    pub(crate) fn admit_foreground_escalation(
        &self,
        session_id: &str,
        owner_approved: bool,
    ) -> Result<Option<ApprovedCall>, ErrorData> {
        if owner_approved {
            // Owner-away mode: an approval never covers a locked workstation.
            self.user_active_gate(FOREGROUND_ESCALATION_ROUTE, Some(session_id))?
                .check_as(true)?;
            tracing::info!(
                code = "USER_ACTIVE_OWNER_APPROVED",
                route = FOREGROUND_ESCALATION_ROUTE,
                session_id,
                "act operation=foreground bypasses the user-activity gate: owner_approved=true with a reason"
            );
            return Ok(Some(ApprovedCall::new(session_id)));
        }
        self.ensure_user_idle(FOREGROUND_ESCALATION_ROUTE, Some(session_id))
            .map(|()| None)
    }
}
