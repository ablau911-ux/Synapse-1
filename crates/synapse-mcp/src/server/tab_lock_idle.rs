//! D6 (#298): a session's CDP tab lock moves to the session that names the
//! tab once the owner made no MCP call for the idle-release window. Pure
//! policy: the m1_tools thin layer reads the facts (owner rows, session
//! registry, in-flight ledger) and applies the plan.

/// Idle-release window in milliseconds. Unset, unparsable or `0` keeps the
/// default.
pub(crate) const IDLE_RELEASE_ENV: &str = "SYNAPSE_TAB_LOCK_IDLE_RELEASE_MS";
pub(crate) const DEFAULT_IDLE_RELEASE_MS: u64 = 10 * 60 * 1000;

/// What the session registry and in-flight ledger say about a lock holder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HolderActivity {
    /// The registry row says the session is closed.
    Closed,
    /// Last MCP call at `last_seen_unix_ms`; `in_flight` = a tool call runs now.
    Seen {
        last_seen_unix_ms: u64,
        in_flight: bool,
    },
    /// No registry row, or a fact could not be read: nothing proves idleness.
    Unknown,
}

/// One session holding owner rows for the requested tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabLockHolder {
    pub session_id: String,
    pub activity: HolderActivity,
}

/// One session asking for a tab.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TabLockRequest<'a> {
    pub requester_session_id: &'a str,
    /// The requester named this exact tab (cdp_target_id).
    pub explicit_target: bool,
    pub holders: &'a [TabLockHolder],
    pub now_unix_ms: u64,
    pub idle_release_ms: u64,
}

/// Why a holder keeps its lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeepReason {
    NoRequester,
    NotExplicit,
    InFlight,
    Active { idle_ms: u64 },
    UnknownActivity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TabLockPlan {
    /// No owner rows: this policy grants nothing (an operator tab stays unowned).
    NoOwner,
    /// The requester already holds the lock.
    Retain,
    /// Every foreign holder is idle or closed: move their rows to the requester.
    Transfer { from: Vec<String> },
    /// `holder` keeps the lock; nothing is mutated.
    Refuse { holder: String, reason: KeepReason },
}

/// The configured idle-release window.
#[must_use]
pub(crate) fn idle_release_ms() -> u64 {
    idle_release_ms_from(std::env::var(IDLE_RELEASE_ENV).ok().as_deref())
}

/// Parses the configured window; unset, unparsable or `0` keeps the default.
#[must_use]
pub(crate) fn idle_release_ms_from(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_IDLE_RELEASE_MS)
}

/// Decides who holds the requested tab. All-or-nothing: one holder that keeps
/// its lock refuses the whole request, so no row moves.
#[must_use]
pub(crate) fn plan_tab_lock(request: &TabLockRequest<'_>) -> TabLockPlan {
    let Some(first) = request.holders.first() else {
        return TabLockPlan::NoOwner;
    };
    if request.requester_session_id.is_empty() {
        return TabLockPlan::Refuse {
            holder: first.session_id.clone(),
            reason: KeepReason::NoRequester,
        };
    }
    if request
        .holders
        .iter()
        .any(|holder| holder.session_id == request.requester_session_id)
    {
        return TabLockPlan::Retain;
    }
    if !request.explicit_target {
        return TabLockPlan::Refuse {
            holder: first.session_id.clone(),
            reason: KeepReason::NotExplicit,
        };
    }
    let mut from: Vec<String> = Vec::new();
    for holder in request.holders {
        if let Some(reason) = keep_reason(
            holder.activity,
            request.now_unix_ms,
            request.idle_release_ms,
        ) {
            return TabLockPlan::Refuse {
                holder: holder.session_id.clone(),
                reason,
            };
        }
        if !from.contains(&holder.session_id) {
            from.push(holder.session_id.clone());
        }
    }
    TabLockPlan::Transfer { from }
}

/// `None` when the holder's lock is released: closed, or no MCP call and no
/// call in flight for the whole window. A last call in the future proves
/// nothing, so it counts as active.
const fn keep_reason(
    activity: HolderActivity,
    now_unix_ms: u64,
    window_ms: u64,
) -> Option<KeepReason> {
    match activity {
        HolderActivity::Closed => None,
        HolderActivity::Unknown => Some(KeepReason::UnknownActivity),
        HolderActivity::Seen {
            in_flight: true, ..
        } => Some(KeepReason::InFlight),
        HolderActivity::Seen {
            last_seen_unix_ms, ..
        } => {
            let idle_ms = now_unix_ms.saturating_sub(last_seen_unix_ms);
            if idle_ms >= window_ms {
                None
            } else {
                Some(KeepReason::Active { idle_ms })
            }
        }
    }
}

/// The refusal for a tab another session keeps: names the owner, why it keeps
/// the lock, the idle-release rule and the way out.
#[must_use]
pub(crate) fn tab_lock_refusal_message(
    tool: &str,
    target_id: &str,
    holder: &str,
    requester: &str,
    reason: KeepReason,
    idle_release_ms: u64,
) -> String {
    let why = match reason {
        KeepReason::NoRequester => "The request has no MCP session id.".to_owned(),
        KeepReason::NotExplicit => {
            "Name this exact tab with cdp_target_id to take over an idle lock.".to_owned()
        }
        KeepReason::InFlight => "The owner has a tool call in flight.".to_owned(),
        KeepReason::UnknownActivity => {
            "The owner's activity cannot be read, so its lock is kept.".to_owned()
        }
        KeepReason::Active { idle_ms } => format!(
            "The owner made an MCP call {} ago; its lock is free in {}.",
            format_window(idle_ms),
            format_window(idle_release_ms.saturating_sub(idle_ms))
        ),
    };
    format!(
        "{tool} refused target {target_id:?}: it is locked by MCP session {holder:?} (requesting session {requester:?}). {why} A tab lock moves to the session that names the tab after {} without MCP calls from its owner. Meanwhile open your own tab with browser_tabs operation=new, or coordinate via target operation=status.",
        format_window(idle_release_ms)
    )
}

/// Whole minutes when exact, otherwise seconds rounded up.
fn format_window(ms: u64) -> String {
    if ms >= 60_000 && ms.is_multiple_of(60_000) {
        let minutes = ms / 60_000;
        if minutes == 1 {
            "1 minute".to_owned()
        } else {
            format!("{minutes} minutes")
        }
    } else {
        let seconds = ms.div_ceil(1000);
        if seconds == 1 {
            "1 second".to_owned()
        } else {
            format!("{seconds} seconds")
        }
    }
}

#[cfg(test)]
#[path = "tab_lock_idle_table_298.rs"]
mod tab_lock_idle_table_298;
