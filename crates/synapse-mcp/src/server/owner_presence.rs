//! Owner-away mode (Phase 5, owner decision 2026-11-19).
//!
//! Real mouse/keyboard input is delivered only while the owner is away: never
//! while the workstation is locked, refused with a reason and a retry time
//! while the owner is active, and stopped at once (release interrupt) when the
//! owner's physical input arrives while an agent holds the foreground input
//! lease. The decisions are pure functions here; the hook, the lock probe and
//! the lease read are the thin OS layer.

use std::sync::atomic::{AtomicU64, Ordering};

use schemars::JsonSchema;
use serde::Serialize;
use synapse_core::error_codes;

/// Default quiet window: physical input younger than this means "active".
pub(crate) const DEFAULT_QUIET_WINDOW_MS: u64 = 60_000;
/// Physical mouse moves are counted at most once per this many milliseconds.
pub(crate) const PHYSICAL_MOVE_COALESCE_MS: u64 = 50;

/// The owner's presence as the real-input gate sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OwnerPresence {
    /// Idle for at least the quiet window: real input may be delivered.
    Away,
    /// Physical input inside the quiet window; retry after `retry_after_ms`.
    Active { retry_after_ms: u64 },
    /// The workstation is locked: retry after unlock plus the quiet window.
    Locked,
    /// The input hook is not running, so presence cannot be known.
    Unknown,
}

/// One resolved presence reading.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PresenceInput {
    pub(crate) monitoring: bool,
    pub(crate) locked: bool,
    /// Age of the newest physical input since the monitor started.
    pub(crate) last_physical_age_ms: Option<u64>,
    /// How long the input monitor has been running.
    pub(crate) monitor_age_ms: Option<u64>,
}

/// Quiet window from the raw config value. Unset or unparsable keeps the
/// default; `0` turns the activity check off (a locked workstation is still
/// refused).
pub(crate) fn quiet_window_from(raw: Option<&str>) -> u64 {
    raw.and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_QUIET_WINDOW_MS)
}

/// The presence verdict for one reading.
pub(crate) const fn owner_presence(input: PresenceInput, quiet_ms: u64) -> OwnerPresence {
    if input.locked {
        return OwnerPresence::Locked;
    }
    if quiet_ms == 0 {
        return OwnerPresence::Away;
    }
    if !input.monitoring {
        return OwnerPresence::Unknown;
    }
    // No physical input since the monitor started (also right after a daemon
    // restart): only a monitor that has watched a full quiet window proves
    // the owner idle.
    let idle_ms = match (input.last_physical_age_ms, input.monitor_age_ms) {
        (Some(age_ms), _) => age_ms,
        (None, Some(monitor_age_ms)) => monitor_age_ms,
        (None, None) => return OwnerPresence::Unknown,
    };
    if idle_ms < quiet_ms {
        OwnerPresence::Active {
            retry_after_ms: quiet_ms - idle_ms,
        }
    } else {
        OwnerPresence::Away
    }
}

/// Whether real input may be delivered now; the refusal is the error code.
/// `approved` is an owner-approved call (`owner_approved=true` + reason): it
/// passes an active or unknown owner, never a locked workstation.
pub(crate) const fn admission(presence: OwnerPresence, approved: bool) -> Result<(), &'static str> {
    match presence {
        OwnerPresence::Locked => Err(error_codes::USER_WORKSTATION_LOCKED),
        _ if approved => Ok(()),
        OwnerPresence::Away => Ok(()),
        OwnerPresence::Active { .. } => Err(error_codes::USER_ACTIVE),
        OwnerPresence::Unknown => Err(error_codes::USER_ACTIVITY_UNKNOWN),
    }
}

/// True when one input event means "the owner returned while an agent drives
/// the real mouse/keyboard": a physical (non-injected) event while an agent
/// session (not the operator hotkey) holds the foreground input lease.
pub(crate) fn owner_returned(event_injected: bool, lease_holder: Option<&str>) -> bool {
    !event_injected
        && matches!(lease_holder, Some(holder) if holder != synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID)
}

/// True when a physical mouse move at `now_ns` is counted, given the last
/// counted move at `last_counted_ns` (0 = none yet).
pub(crate) const fn physical_move_due(now_ns: u64, last_counted_ns: u64, gap_ms: u64) -> bool {
    // A clock that went backwards restarts the coalescing instead of muting
    // every move until it catches up.
    now_ns < last_counted_ns || now_ns - last_counted_ns >= gap_ms.saturating_mul(1_000_000)
}

/// True when a foreground action that started at `action_start_ns` failed
/// because the owner-return stop fired while it ran: the release interrupt
/// surfaces as the operator-hotkey or foreground-lost code, and the stop is
/// recorded at or after the action's start.
pub(crate) fn stopped_by_owner_return(
    error_code: Option<&str>,
    last_owner_return_ns: Option<u64>,
    action_start_ns: u64,
) -> bool {
    matches!(
        error_code,
        Some(error_codes::SAFETY_OPERATOR_HOTKEY_FIRED | error_codes::ACTION_FOREGROUND_LOST)
    ) && last_owner_return_ns.is_some_and(|ns| ns >= action_start_ns)
}

/// Wall-clock ns of the last owner-return stop (0 = none since start).
static LAST_OWNER_RETURN_NS: AtomicU64 = AtomicU64::new(0);

/// Hook side of the owner-return stop: called for every keyboard/mouse event
/// the input hook sees (mouse moves coalesced). A physical event while an
/// agent holds the foreground input lease raises the release interrupt, which
/// the action backend honors before its next input step.
pub(crate) fn observe_input_event(event_injected: bool, ts_ns: u64) {
    if event_injected {
        return;
    }
    let holder = synapse_action::lease::status().owner_session_id;
    if owner_returned(event_injected, holder.as_deref()) {
        synapse_action::request_release_interrupt();
        LAST_OWNER_RETURN_NS.store(ts_ns, Ordering::Release);
        tracing::warn!(
            code = error_codes::USER_RETURNED,
            lease_holder = holder.as_deref().unwrap_or(""),
            "owner physical input while an agent held the foreground input lease; release interrupt raised"
        );
    }
}

/// When the owner-return stop last fired, if it has since the daemon started.
pub(crate) fn last_owner_return_ns() -> Option<u64> {
    match LAST_OWNER_RETURN_NS.load(Ordering::Acquire) {
        0 => None,
        ns => Some(ns),
    }
}

#[cfg(test)]
#[path = "owner_presence_table_away.rs"]
mod owner_presence_table_away;
