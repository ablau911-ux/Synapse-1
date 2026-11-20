//! #147: a crashed MCP client never sends HTTP DELETE, and HTTP sessions stay
//! "live" for the 24 h keep-alive, so an owned browser seat (a private Chromium
//! process + profile) outlived its client for up to a day and no other session
//! may close it. Seat owners get a shorter MCP-idle reclaim: after
//! `SYNAPSE_SEAT_OWNER_IDLE_RECLAIM_SECS` (default 30 min, 0 disables) with no
//! MCP activity, ONLY the owner's seats are closed; the session itself, its
//! leases, targets and clipboard stay (a connected-but-idle human client must
//! not lose its session). An open SSE stream counts as activity. Spawned agents
//! are excluded: their liveness is a process probe, not MCP recency (#1238).

use std::{collections::BTreeSet, sync::OnceLock};

use super::session_registry::SessionRegistryRead;

pub(crate) const SEAT_OWNER_IDLE_REASON: &str = "seat_owner_idle";
const SEAT_OWNER_IDLE_ENV: &str = "SYNAPSE_SEAT_OWNER_IDLE_RECLAIM_SECS";
const DEFAULT_SEAT_OWNER_IDLE_SECS: u64 = 30 * 60;

/// Configured idle limit in milliseconds; `None` when reclaim is disabled.
/// An unparsable value keeps the default and is reported.
pub(crate) fn seat_owner_idle_limit_ms(raw: Option<&str>) -> Option<u64> {
    let secs = match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => DEFAULT_SEAT_OWNER_IDLE_SECS,
        Some(value) => value.parse::<u64>().unwrap_or_else(|_| {
            tracing::warn!(
                code = "MCP_SEAT_OWNER_IDLE_CONFIG_INVALID",
                value,
                default_secs = DEFAULT_SEAT_OWNER_IDLE_SECS,
                "unparsable {SEAT_OWNER_IDLE_ENV}; using default"
            );
            DEFAULT_SEAT_OWNER_IDLE_SECS
        }),
    };
    (secs > 0).then(|| secs.saturating_mul(1000))
}

/// Env-configured limit, read and validated once per process so an invalid
/// value warns once instead of on every sweep.
pub(crate) fn seat_owner_idle_limit_from_env() -> Option<u64> {
    static LIMIT: OnceLock<Option<u64>> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        seat_owner_idle_limit_ms(std::env::var(SEAT_OWNER_IDLE_ENV).ok().as_deref())
    })
}

/// The registry facts the reclaim decision needs.
#[derive(Clone, Debug)]
pub(crate) struct SessionActivity<'a> {
    pub session_id: &'a str,
    pub is_spawned_agent: bool,
    pub is_closed: bool,
    pub last_seen_ms_ago: u64,
}

impl<'a> From<&'a SessionRegistryRead> for SessionActivity<'a> {
    fn from(read: &'a SessionRegistryRead) -> Self {
        Self {
            session_id: &read.session_id,
            is_spawned_agent: read.spawned_agent.is_some(),
            is_closed: read.closed_at_unix_ms.is_some(),
            last_seen_ms_ago: read.last_seen_ms_ago,
        }
    }
}

/// True when a seat owner's session has been MCP-idle past the limit.
/// Unknown sessions are left to the ordinary stale scan.
pub(crate) fn seat_owner_idle_expired<'a>(
    owner: &str,
    activity: impl IntoIterator<Item = SessionActivity<'a>>,
    limit_ms: u64,
) -> bool {
    activity.into_iter().any(|session| {
        session.session_id == owner
            && !session.is_spawned_agent
            && !session.is_closed
            && session.last_seen_ms_ago > limit_ms
    })
}

/// Seat owners (deduplicated, in first-seen order) that are MCP-idle past the
/// limit and have no open SSE stream: their seats are reclaimed, nothing else.
pub(crate) fn idle_seat_owners<'a>(
    owners: impl IntoIterator<Item = &'a str>,
    activity: &[SessionActivity<'_>],
    streaming_owners: &BTreeSet<String>,
    limit_ms: u64,
) -> Vec<String> {
    let mut selected: Vec<String> = Vec::new();
    for owner in owners {
        if streaming_owners.contains(owner) || selected.iter().any(|seen| seen == owner) {
            continue;
        }
        if seat_owner_idle_expired(owner, activity.iter().cloned(), limit_ms) {
            selected.push(owner.to_owned());
        }
    }
    selected
}

#[cfg(test)]
#[path = "tests/seat_owner_idle_147.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/seat_owner_idle_147b.rs"]
mod tests_147b;
