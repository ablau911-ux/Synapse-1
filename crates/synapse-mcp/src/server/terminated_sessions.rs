//! Bounded registry of recently terminated MCP session ids (#14).
//!
//! The HTTP layer consults it to answer a request on a torn-down session with
//! the distinct `session_terminated` diagnostic instead of the generic
//! unknown/expired one. It only needs to remember *recent* terminations, so it
//! is a FIFO ring: once full, the oldest id is forgotten (that session then
//! reads as unknown/expired, which is still a 404 with the same recovery).

use std::collections::{HashSet, VecDeque};

use anyhow::{Context, bail};

pub(crate) const CAPACITY_ENV: &str = "SYNAPSE_TERMINATED_SESSION_CAPACITY";
const DEFAULT_CAPACITY: usize = 4096;
pub(crate) const HARD_MAX_CAPACITY: usize = 4096;

#[derive(Debug)]
pub(crate) struct TerminatedSessions {
    capacity: usize,
    order: VecDeque<String>,
    members: HashSet<String>,
    evicted_total: u64,
}

/// Occupancy readback for logs and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TerminatedSessionsStats {
    pub len: usize,
    pub capacity: usize,
    pub evicted_total: u64,
}

impl Default for TerminatedSessions {
    fn default() -> Self {
        let capacity = match std::env::var(CAPACITY_ENV) {
            Ok(raw) => match Self::parse_capacity(&raw) {
                Ok(cap) => cap,
                Err(error) => {
                    tracing::warn!(
                        code = "TERMINATED_SESSION_CAPACITY_INVALID",
                        env = CAPACITY_ENV,
                        value = %raw,
                        error = %format!("{error:#}"),
                        "invalid terminated session capacity override; using default"
                    );
                    DEFAULT_CAPACITY
                }
            },
            Err(std::env::VarError::NotPresent) => DEFAULT_CAPACITY,
            Err(error) => {
                tracing::warn!(
                    code = "TERMINATED_SESSION_CAPACITY_INVALID",
                    env = CAPACITY_ENV,
                    %error,
                    "unreadable terminated session capacity override; using default"
                );
                DEFAULT_CAPACITY
            }
        };
        Self::with_capacity(capacity)
    }
}

impl TerminatedSessions {
    pub(crate) fn parse_capacity(raw: &str) -> anyhow::Result<usize> {
        let value = raw.trim();
        let capacity = value
            .parse::<usize>()
            .with_context(|| format!("invalid integer {value:?}"))?;
        if capacity == 0 {
            bail!("terminated session capacity must be greater than zero");
        }
        if capacity > HARD_MAX_CAPACITY {
            bail!(
                "terminated session capacity must not exceed {HARD_MAX_CAPACITY}, got {capacity}"
            );
        }
        Ok(capacity)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, HARD_MAX_CAPACITY),
            order: VecDeque::new(),
            members: HashSet::new(),
            evicted_total: 0,
        }
    }

    pub(crate) fn contains(&self, session_id: &str) -> bool {
        self.members.contains(session_id)
    }

    /// Record a termination. Returns `true` when the id was newly added; a
    /// duplicate is a no-op and keeps its original age.
    pub(crate) fn insert(&mut self, session_id: String) -> bool {
        if self.members.contains(&session_id) {
            return false;
        }
        while self.order.len() >= self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.members.remove(&oldest);
            self.evicted_total = self.evicted_total.saturating_add(1);
        }
        self.members.insert(session_id.clone());
        self.order.push_back(session_id);
        true
    }

    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    pub(crate) fn stats(&self) -> TerminatedSessionsStats {
        TerminatedSessionsStats {
            len: self.len(),
            capacity: self.capacity,
            evicted_total: self.evicted_total,
        }
    }
}
