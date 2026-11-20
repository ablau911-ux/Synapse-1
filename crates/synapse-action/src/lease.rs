//! Process-global, renewable, TTL-bounded **input lease** for the real
//! foreground/cursor/keyboard/clipboard (epic #719).
//!
//! Synapse runs one daemon serving many MCP sessions (one per agent terminal).
//! Background action tiers (CDP, UIA control patterns, `PostMessage`) touch no
//! shared OS input state and run in parallel without ever taking this lease.
//! Only a *leased foreground* action — a real `SendInput`/`SetPhysicalCursorPos`
//! that steals the single physical cursor/foreground — must own the lease.
//!
//! Semantics are **refuse, not block**: a contended `try_acquire` returns
//! [`LeaseOutcome::Busy`] with the current holder and a retry hint rather than
//! waiting, so an agent never deadlocks on another agent's foreground action.
//! The lease auto-expires lazily on TTL lapse, but a lapsed owner leaves a
//! pending cleanup record that blocks new acquisition until that session's
//! held-input ledger is drained. Session disconnect releases inputs and lease
//! through the same cleanup path, so a crashed agent cannot leave foreground
//! input stuck behind an unowned lease.
//!
//! The lock-free-at-rest static mirrors the [`crate::hotkey`] module's
//! process-global state pattern. The guard is a plain `std::sync::Mutex` held
//! only for the O(1) critical section — never across an `.await` or an action
//! emit — so [`status`] never blocks a health probe.

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard, OnceLock, PoisonError},
    time::{Duration, Instant},
};

use synapse_core::error_codes;

/// Default lease lifetime when a caller does not specify one.
pub const DEFAULT_LEASE_TTL_MS: u64 = 5_000;
/// Minimum acceptable lease lifetime (clamped by [`ttl_from_ms`]).
pub const MIN_LEASE_TTL_MS: u64 = 100;
/// Maximum acceptable lease lifetime (clamped by [`ttl_from_ms`]).
pub const MAX_LEASE_TTL_MS: u64 = 30_000;
/// Synthetic holder used when the operator panic hotkey preempts agents.
pub const OPERATOR_LEASE_OWNER_SESSION_ID: &str = "__operator__";
/// How long the operator owns the real-input resource after panic preemption.
pub const OPERATOR_PREEMPT_LEASE_TTL_MS: u64 = MAX_LEASE_TTL_MS;

static EMERGENCY_RESERVATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Fences physical grants while emergency cleanup crosses asynchronous work.
#[must_use]
pub struct EmergencyReservation {
    _private: (),
}

pub fn reserve_emergency_cleanup() -> EmergencyReservation {
    EMERGENCY_RESERVATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    EmergencyReservation { _private: () }
}

#[must_use]
pub fn emergency_cleanup_pending() -> bool {
    EMERGENCY_RESERVATIONS.load(std::sync::atomic::Ordering::SeqCst) != 0
}

impl Drop for EmergencyReservation {
    fn drop(&mut self) {
        EMERGENCY_RESERVATIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn next_lease_generation() -> String {
    static EPOCH: OnceLock<String> = OnceLock::new();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let epoch = EPOCH.get_or_init(|| {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        format!("{}:{started}", std::process::id())
    });
    format!(
        "{epoch}:{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Internal lease record. Stored behind the process-global mutex.
#[derive(Clone, Debug)]
struct InputLease {
    generation: String,
    owner_session_id: String,
    acquired_at: Instant,
    renewed_at: Instant,
    ttl: Duration,
    /// `true` while the lease exists only because a foreground action
    /// auto-acquired it (#205). Such a lease is released by that action; an
    /// explicit acquire, a handoff or operator preemption clears the flag so the
    /// action's release becomes a no-op.
    auto: bool,
}

impl InputLease {
    fn is_expired(&self, now: Instant) -> bool {
        now.duration_since(self.renewed_at) >= self.ttl
    }

    fn expires_in(&self, now: Instant) -> Duration {
        self.ttl.saturating_sub(now.duration_since(self.renewed_at))
    }

    fn status(&self, now: Instant) -> LeaseStatus {
        LeaseStatus {
            generation: Some(self.generation.clone()),
            held: true,
            owner_session_id: Some(self.owner_session_id.clone()),
            acquired_at_ms_ago: Some(duration_ms(now.duration_since(self.acquired_at))),
            renewed_at_ms_ago: Some(duration_ms(now.duration_since(self.renewed_at))),
            ttl_ms: Some(duration_ms(self.ttl)),
            expires_in_ms: Some(duration_ms(self.expires_in(now))),
        }
    }
}

/// Serializable snapshot of the lease, used by MCP tools and `/health`.
///
/// All time fields are milliseconds. When `held` is `false` every optional
/// field is `None`, so an unheld lease serializes to an unambiguous "nobody
/// owns the foreground" shape.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeaseStatus {
    /// Process/acquisition identity for continuity; not part of the wire API.
    #[serde(skip)]
    pub generation: Option<String>,
    pub held: bool,
    pub owner_session_id: Option<String>,
    pub acquired_at_ms_ago: Option<u64>,
    pub renewed_at_ms_ago: Option<u64>,
    pub ttl_ms: Option<u64>,
    pub expires_in_ms: Option<u64>,
}

impl LeaseStatus {
    /// The canonical "nobody holds the lease" snapshot.
    #[must_use]
    pub const fn unheld() -> Self {
        Self {
            generation: None,
            held: false,
            owner_session_id: None,
            acquired_at_ms_ago: None,
            renewed_at_ms_ago: None,
            ttl_ms: None,
            expires_in_ms: None,
        }
    }
}

/// Result of [`try_acquire`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseOutcome {
    /// The lease was free (or expired) and is now owned by the caller.
    Acquired(LeaseStatus),
    /// The caller already owned the lease; its TTL window was refreshed.
    Renewed(LeaseStatus),
    /// Another live session owns the lease. The caller is refused (not blocked).
    Busy {
        holder: LeaseStatus,
        retry_after_ms: u64,
    },
    /// A lapsed holder must have its per-session held-input ledger drained
    /// before any session can acquire the real-input lease.
    CleanupPending {
        expired: LeaseStatus,
        retry_after_ms: u64,
    },
}

/// Before/after snapshots from an atomic holder-to-peer handoff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseHandoff {
    pub prior: LeaseStatus,
    pub current: LeaseStatus,
}

/// Error returned by input lease operations.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    #[error("emergency input cleanup is pending; retry after 100ms")]
    CleanupBusy { holder: Option<String> },
    #[error("input lease not held by session {session_id:?}; current holder {holder:?}")]
    NotHeld {
        session_id: String,
        holder: Option<String>,
    },
}

impl LeaseError {
    /// Stable error code for MCP surfacing.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CleanupBusy { .. } => error_codes::ACTION_FOREGROUND_LEASE_BUSY,
            Self::NotHeld { .. } => error_codes::ACTION_FOREGROUND_LEASE_NOT_HELD,
        }
    }
}

fn slot() -> &'static Mutex<Option<InputLease>> {
    static LEASE: OnceLock<Mutex<Option<InputLease>>> = OnceLock::new();
    LEASE.get_or_init(|| Mutex::new(None))
}

fn expired_cleanup_slot() -> &'static Mutex<BTreeMap<String, LeaseStatus>> {
    static EXPIRED_CLEANUP: OnceLock<Mutex<BTreeMap<String, LeaseStatus>>> = OnceLock::new();
    EXPIRED_CLEANUP.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Locks the lease slot, recovering from a poisoned mutex rather than panicking:
/// a foreground lease that panicked mid-action must still be reclaimable.
fn lock() -> MutexGuard<'static, Option<InputLease>> {
    slot().lock().unwrap_or_else(PoisonError::into_inner)
}

fn lock_expired_cleanup() -> MutexGuard<'static, BTreeMap<String, LeaseStatus>> {
    expired_cleanup_slot()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Clamps a caller-supplied TTL in milliseconds into the accepted range.
#[must_use]
pub fn ttl_from_ms(ms: u64) -> Duration {
    Duration::from_millis(ms.clamp(MIN_LEASE_TTL_MS, MAX_LEASE_TTL_MS))
}

/// Drops the stored lease if its TTL has lapsed and records the expired owner
/// until its held-input ledger has been drained. Caller holds the lease lock.
fn expire_if_lapsed(guard: &mut Option<InputLease>, now: Instant) -> Option<LeaseStatus> {
    let expired = guard
        .as_ref()
        .filter(|lease| lease.is_expired(now))
        .map(|lease| lease.status(now));
    if let Some(status) = expired.clone() {
        *guard = None;
        remember_expired_cleanup(&status);
    }
    expired
}

fn remember_expired_cleanup(status: &LeaseStatus) {
    let Some(owner_session_id) = status.owner_session_id.clone() else {
        return;
    };
    if owner_session_id == OPERATOR_LEASE_OWNER_SESSION_ID {
        return;
    }
    {
        let mut pending = lock_expired_cleanup();
        pending
            .entry(owner_session_id.clone())
            .or_insert_with(|| status.clone());
    }
    tracing::warn!(
        code = error_codes::ACTION_FOREGROUND_LEASE_BUSY,
        owner_session_id,
        ttl_ms = status.ttl_ms,
        acquired_at_ms_ago = status.acquired_at_ms_ago,
        renewed_at_ms_ago = status.renewed_at_ms_ago,
        "readback=input_lease edge=expired pending_session_input_cleanup"
    );
}

fn first_pending_expired_cleanup() -> Option<LeaseStatus> {
    lock_expired_cleanup().values().next().cloned()
}

/// Reads expired lease owners whose held-input ledgers must be drained before
/// the foreground lease can be granted again.
#[must_use]
pub fn expired_cleanup_snapshot() -> Vec<LeaseStatus> {
    lock_expired_cleanup().values().cloned().collect()
}

/// Marks a previously expired owner's held-input cleanup as complete.
///
/// Returns `true` when a pending expired owner record was removed.
#[must_use]
pub fn complete_expired_cleanup(session_id: &str) -> bool {
    lock_expired_cleanup().remove(session_id).is_some()
}

/// Attempts to acquire (or renew) the lease for `session_id` as an explicit
/// holder (see [`try_acquire_explicit`]).
///
/// Refuse-not-block: if another live session holds it, returns
/// [`LeaseOutcome::Busy`] immediately with the holder and a retry hint.
#[must_use]
pub fn try_acquire(session_id: &str, ttl: Duration) -> LeaseOutcome {
    try_acquire_explicit(session_id, ttl).0
}

/// Explicit acquire (`lease_acquire`). Also reports whether the caller took
/// over a lease that one of its own foreground actions had auto-acquired
/// ([`try_acquire_auto`]): the lease then stops being released by that action
/// and the caller must treat the call as a fresh acquisition (#205).
#[must_use]
pub fn try_acquire_explicit(session_id: &str, ttl: Duration) -> (LeaseOutcome, bool) {
    acquire(session_id, ttl, AcquireKind::Explicit)
}

/// Whether a live automatic hold can be promoted by its own session (#205).
#[must_use]
pub fn owns_automatic_lease(session_id: &str) -> bool {
    let now = Instant::now();
    lock().as_ref().is_some_and(|lease| {
        lease.auto && lease.owner_session_id == session_id && !lease.is_expired(now)
    })
}

/// Acquire/renew on behalf of a foreground action. A lease it newly acquires is
/// marked auto, so [`release_auto`] ends it when the action finishes; renewing
/// a lease that is already explicit leaves it explicit.
#[must_use]
pub fn try_acquire_auto(session_id: &str, ttl: Duration) -> LeaseOutcome {
    acquire(session_id, ttl, AcquireKind::Auto).0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquireKind {
    Explicit,
    Auto,
}

fn acquire(session_id: &str, ttl: Duration, kind: AcquireKind) -> (LeaseOutcome, bool) {
    let now = Instant::now();
    let mut guard = lock();
    if emergency_cleanup_pending() {
        return (
            LeaseOutcome::Busy {
                holder: guard
                    .as_ref()
                    .map_or_else(LeaseStatus::unheld, |lease| lease.status(now)),
                retry_after_ms: 100,
            },
            false,
        );
    }
    if let Some(expired) = expire_if_lapsed(&mut guard, now) {
        return (
            LeaseOutcome::CleanupPending {
                expired,
                retry_after_ms: 100,
            },
            false,
        );
    }
    if let Some(expired) = first_pending_expired_cleanup() {
        return (
            LeaseOutcome::CleanupPending {
                expired,
                retry_after_ms: 100,
            },
            false,
        );
    }
    let mut converted_from_auto = false;
    let outcome = match guard.as_mut() {
        Some(lease) if lease.owner_session_id == session_id => {
            // #205: an action's auto-renew over the session's own explicit
            // lease only ever extends it; it never shortens the explicit TTL.
            let keep_explicit_expiry =
                kind == AcquireKind::Auto && !lease.auto && lease.expires_in(now) >= ttl;
            if !keep_explicit_expiry {
                lease.renewed_at = now;
                lease.ttl = ttl;
            }
            if kind == AcquireKind::Explicit && lease.auto {
                lease.generation = next_lease_generation();
                lease.auto = false;
                converted_from_auto = true;
            }
            LeaseOutcome::Renewed(lease.status(now))
        }
        Some(lease) => {
            let holder = lease.status(now);
            let retry_after_ms = duration_ms(lease.expires_in(now));
            LeaseOutcome::Busy {
                holder,
                retry_after_ms,
            }
        }
        None => {
            let lease = InputLease {
                generation: next_lease_generation(),
                owner_session_id: session_id.to_owned(),
                acquired_at: now,
                renewed_at: now,
                ttl,
                auto: kind == AcquireKind::Auto,
            };
            let status = lease.status(now);
            *guard = Some(lease);
            LeaseOutcome::Acquired(status)
        }
    };
    drop(guard);
    (outcome, converted_from_auto)
}

/// Refreshes the TTL window for the holder. Errors if `session_id` is not the holder.
///
/// # Errors
///
/// Returns [`LeaseError::NotHeld`] when the lease is unheld, expired, or owned
/// by a different session.
pub fn renew(session_id: &str, ttl: Option<Duration>) -> Result<LeaseStatus, LeaseError> {
    let now = Instant::now();
    let mut guard = lock();
    if emergency_cleanup_pending() {
        return Err(LeaseError::CleanupBusy {
            holder: guard.as_ref().map(|lease| lease.owner_session_id.clone()),
        });
    }
    let _expired = expire_if_lapsed(&mut guard, now);
    match guard.as_mut() {
        Some(lease) if lease.owner_session_id == session_id => {
            lease.renewed_at = now;
            if let Some(ttl) = ttl {
                lease.ttl = ttl;
            }
            Ok(lease.status(now))
        }
        other => Err(LeaseError::NotHeld {
            session_id: session_id.to_owned(),
            holder: other.map(|lease| lease.owner_session_id.clone()),
        }),
    }
}

/// Releases the caller's lease and returns the unheld status plus a flag
/// indicating whether it was already unheld.
///
/// # Errors
///
/// Returns [`LeaseError::NotHeld`] when another session holds the lease.
pub fn release(session_id: &str) -> Result<(LeaseStatus, bool), LeaseError> {
    let now = Instant::now();
    let mut guard = lock();
    let _expired = expire_if_lapsed(&mut guard, now);
    let holder = guard.as_ref().map(|lease| lease.owner_session_id.as_str());
    let result = match lease_release_decision(holder, session_id) {
        LeaseReleaseDecision::Release => {
            *guard = None;
            Ok((LeaseStatus::unheld(), false))
        }
        LeaseReleaseDecision::AlreadyReleased => Ok((LeaseStatus::unheld(), true)),
        LeaseReleaseDecision::NotHolder => Err(LeaseError::NotHeld {
            session_id: session_id.to_owned(),
            holder: holder.map(str::to_owned),
        }),
    };
    drop(guard);
    result
}

/// #2325: what a `release` call by `session_id` does, given the live holder
/// (after lapsed leases were expired).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseReleaseDecision {
    /// The caller holds the lease: free it.
    Release,
    /// Nobody holds the lease (never taken, already released, or expired):
    /// success, nothing to do.
    AlreadyReleased,
    /// Another session holds the lease: refuse, the lease is not the caller's.
    NotHolder,
}

/// Pure release rule (#2325).
#[must_use]
pub fn lease_release_decision(holder: Option<&str>, session_id: &str) -> LeaseReleaseDecision {
    match holder {
        Some(owner) if owner == session_id => LeaseReleaseDecision::Release,
        Some(_) => LeaseReleaseDecision::NotHolder,
        None => LeaseReleaseDecision::AlreadyReleased,
    }
}

#[cfg(test)]
#[path = "lease_release_table_2325.rs"]
mod lease_release_table_2325;

/// Releases the lease on behalf of a foreground action that auto-acquired it.
///
/// Returns `Ok(true)` when the lease was released and `Ok(false)` when the
/// lease is unheld or the caller's lease has become explicit (#205).
///
/// # Errors
///
/// Returns [`LeaseError::NotHeld`] when another session holds the lease.
pub fn release_auto(session_id: &str) -> Result<bool, LeaseError> {
    let now = Instant::now();
    let mut guard = lock();
    let _expired = expire_if_lapsed(&mut guard, now);
    let holder = guard.as_ref().map(|lease| lease.owner_session_id.as_str());
    let result = match lease_release_decision(holder, session_id) {
        LeaseReleaseDecision::Release if guard.as_ref().is_some_and(|lease| lease.auto) => {
            *guard = None;
            Ok(true)
        }
        LeaseReleaseDecision::Release | LeaseReleaseDecision::AlreadyReleased => Ok(false),
        LeaseReleaseDecision::NotHolder => Err(LeaseError::NotHeld {
            session_id: session_id.to_owned(),
            holder: holder.map(str::to_owned),
        }),
    };
    drop(guard);
    result
}

/// Marks a lease owned by `session_id` auto again, so the foreground action
/// that still holds a guard keeps releasing it. For rolling back an explicit
/// takeover that could not be made durable. Returns `true` when `session_id`
/// holds the lease.
#[must_use]
pub fn restore_auto_if_owner(session_id: &str) -> bool {
    let mut guard = lock();
    let mut restored = false;
    if let Some(lease) = guard.as_mut()
        && lease.owner_session_id == session_id
    {
        lease.auto = true;
        restored = true;
    }
    drop(guard);
    restored
}

/// Transfers the currently held lease from `from_session_id` directly to
/// `to_session_id` without releasing it into the free pool.
///
/// # Errors
///
/// Returns [`LeaseError::NotHeld`] when `from_session_id` does not currently
/// hold the lease. Lapsed holders are expired before the check, so a handoff
/// cannot revive an expired lease without the normal held-input cleanup path.
pub fn handoff(
    from_session_id: &str,
    to_session_id: &str,
    ttl: Duration,
) -> Result<LeaseHandoff, LeaseError> {
    let now = Instant::now();
    let mut guard = lock();
    if emergency_cleanup_pending() {
        return Err(LeaseError::CleanupBusy {
            holder: guard.as_ref().map(|lease| lease.owner_session_id.clone()),
        });
    }
    let _expired = expire_if_lapsed(&mut guard, now);
    match guard.as_mut() {
        Some(lease) if lease.owner_session_id == from_session_id => {
            let prior = lease.status(now);
            lease.generation = next_lease_generation();
            to_session_id.clone_into(&mut lease.owner_session_id);
            lease.acquired_at = now;
            lease.renewed_at = now;
            lease.ttl = ttl;
            // A handed-off lease belongs to the recipient explicitly; no action
            // of the previous holder may release it.
            lease.auto = false;
            let current = lease.status(now);
            Ok(LeaseHandoff { prior, current })
        }
        other => Err(LeaseError::NotHeld {
            session_id: from_session_id.to_owned(),
            holder: other.map(|lease| lease.owner_session_id.clone()),
        }),
    }
}

/// Releases the lease iff `session_id` currently holds it. Infallible; for
/// session-disconnect/expiry cleanup where a non-owner call must be a no-op.
///
/// Returns `true` when a lease owned by `session_id` was released.
#[must_use]
pub fn release_if_owner(session_id: &str) -> bool {
    let mut guard = lock();
    let released = guard
        .as_ref()
        .is_some_and(|lease| lease.owner_session_id == session_id);
    if released {
        *guard = None;
    }
    drop(guard);
    released
}

/// Operator override: transfers the lease to the operator (e.g. panic hotkey).
///
/// Returns the prior holder's snapshot, if any, so the preemption can be logged.
pub fn force_preempt(reason: &str) -> Option<LeaseStatus> {
    let now = Instant::now();
    let mut guard = lock();
    let prior = guard.as_ref().map(|lease| lease.status(now));
    if let Some(prior) = &prior {
        tracing::warn!(
            code = error_codes::ACTION_FOREGROUND_LEASE_BUSY,
            reason,
            prior_owner = ?prior.owner_session_id,
            "input lease force-preempted by operator"
        );
    }
    *guard = Some(InputLease {
        generation: next_lease_generation(),
        owner_session_id: OPERATOR_LEASE_OWNER_SESSION_ID.to_owned(),
        acquired_at: now,
        renewed_at: now,
        ttl: ttl_from_ms(OPERATOR_PREEMPT_LEASE_TTL_MS),
        auto: false,
    });
    prior
}

/// Unconditionally clears the process-global lease.
///
/// This is intentionally separate from [`force_preempt`]: operator preemption
/// must leave a visible, bounded operator holder so agents fail closed instead
/// of immediately reacquiring the foreground resource.
pub fn force_clear(reason: &str) -> Option<LeaseStatus> {
    let now = Instant::now();
    let mut guard = lock();
    let prior = guard.as_ref().map(|lease| lease.status(now));
    *guard = None;
    drop(guard);
    lock_expired_cleanup().clear();
    if let Some(prior) = &prior {
        tracing::info!(
            reason,
            prior_owner = ?prior.owner_session_id,
            "input lease force-cleared"
        );
    }
    prior
}

/// Clears the lease only when the named session is still the live owner.
///
/// This operator-control primitive is race-safe against clearing a lease that
/// moved to a different session after the UI read its before state.
pub fn force_clear_if_owner(session_id: &str, reason: &str) -> Option<LeaseStatus> {
    let now = Instant::now();
    let mut guard = lock();
    let _expired = expire_if_lapsed(&mut guard, now);
    let prior = guard
        .as_ref()
        .filter(|lease| lease.owner_session_id == session_id)
        .map(|lease| lease.status(now));
    if prior.is_some() {
        *guard = None;
    }
    drop(guard);
    if let Some(prior) = &prior {
        tracing::warn!(
            reason,
            prior_owner = ?prior.owner_session_id,
            "input lease force-cleared by owner-guarded operator override"
        );
        lock_expired_cleanup().clear();
    }
    prior
}

/// Clears only the exact emergency acquisition represented by `expected`.
#[must_use]
pub fn force_clear_if_generation(expected: &LeaseStatus, reason: &str) -> Option<LeaseStatus> {
    let mut guard = lock();
    let prior = guard
        .as_ref()
        .filter(|lease| {
            expected.generation.as_deref() == Some(lease.generation.as_str())
                && expected.owner_session_id.as_deref() == Some(lease.owner_session_id.as_str())
        })
        .map(|lease| lease.status(Instant::now()));
    if prior.is_some() {
        *guard = None;
        tracing::warn!(reason, generation = ?expected.generation, "input lease exact generation force-cleared");
    }
    prior
}

/// Reads the current lease snapshot, lazily expiring a lapsed lease first.
///
/// Never blocks beyond the O(1) critical section; safe for `/health`.
#[must_use]
pub fn status() -> LeaseStatus {
    let now = Instant::now();
    let mut guard = lock();
    let _expired = expire_if_lapsed(&mut guard, now);
    guard
        .as_ref()
        .map_or_else(LeaseStatus::unheld, |lease| lease.status(now))
}
