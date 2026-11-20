//! The single in-flight `act operation=foreground` escalation (#53).
//!
//! There is one real OS foreground and one input lease, so escalations are
//! serialized daemon-wide and a second one is refused rather than queued.
//! While a session's escalation is in flight, that session's other profile,
//! release and act operations are refused too: otherwise a concurrent
//! release and reacquire or a profile set could be undone or released by the
//! escalation's unwind, and a concurrent action would run under the
//! escalation's temporary profile and lease.
//!
//! Each session has a fence (a read/write lock) held for the whole operation
//! on both sides: an escalation holds its session's fence exclusively from
//! claim to unwind, and a standalone operation holds it shared until it
//! finishes, so neither can start while the other is part-way through.
//! Fences are per session, so one session's operations never block another
//! session's escalation.
//! Explicit acquisition may join its own lease under the promotion gate;
//! unwind then preserves that acknowledged hold for the caller to release.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, PoisonError},
};

use synapse_core::error_codes;
use tokio::sync::{
    Mutex, MutexGuard, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
};

use super::{ErrorData, mcp_error};

#[derive(Clone, Default)]
struct Fence {
    operation: Arc<RwLock<()>>,
    promotion: Arc<Mutex<LeasePromotionState>>,
}

#[derive(Default)]
struct LeasePromotionState {
    promoted: bool,
    unwinding: bool,
}

static ESCALATION: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static FENCES: LazyLock<std::sync::Mutex<HashMap<String, Fence>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// Held for the whole acquire → escalate → act → unwind transaction.
pub(crate) struct ActForegroundSlot {
    session_id: String,
    exclusive: Option<OwnedRwLockWriteGuard<()>>,
    _escalation: MutexGuard<'static, ()>,
}

impl ActForegroundSlot {
    /// Claims the slot for `session_id`. Refuses while another escalation is
    /// in flight or this session has another operation part-way through.
    pub(crate) fn claim(session_id: &str) -> Result<Self, ErrorData> {
        let escalation = ESCALATION.try_lock().map_err(|_busy| {
            busy("another act operation=foreground escalation is in progress; retry after it completes")
        })?;
        let exclusive = claim_fence(session_id).inspect_err(|_error| {
            // The failed attempt's fence clone is gone; drop the entry if
            // the operation holding it already finished and pruned early.
            prune(session_id);
        })?;
        Ok(Self {
            session_id: session_id.to_owned(),
            exclusive: Some(exclusive),
            _escalation: escalation,
        })
    }
}

fn claim_fence(session_id: &str) -> Result<OwnedRwLockWriteGuard<()>, ErrorData> {
    let fence = fence(session_id);
    let refused = || {
        busy(
            "this session has a lease/profile/act operation in progress; retry act operation=foreground after it completes",
        )
    };
    let mut promotion = fence.promotion.try_lock_owned().map_err(|_| refused())?;
    let exclusive = fence.operation.try_write_owned().map_err(|_| refused())?;
    *promotion = LeasePromotionState::default();
    Ok(exclusive)
}

/// Serializes explicit joining with fence claims and foreground unwind (#205).
pub(crate) struct ForegroundLeasePromotion {
    session_id: String,
    guard: Option<OwnedMutexGuard<LeasePromotionState>>,
}

impl ForegroundLeasePromotion {
    pub(crate) fn can_join(&self) -> bool {
        self.guard.as_ref().is_some_and(|state| !state.unwinding)
    }

    pub(crate) fn promote(&mut self) {
        if let Some(state) = &mut self.guard {
            state.promoted = true;
        }
    }

    pub(crate) fn start_unwind(&mut self) -> bool {
        self.guard.as_mut().is_some_and(|state| {
            state.unwinding = true;
            state.promoted
        })
    }
}

impl Drop for ForegroundLeasePromotion {
    fn drop(&mut self) {
        drop(self.guard.take());
        prune(&self.session_id);
    }
}

pub(crate) async fn lock_lease_promotion(session_id: &str) -> ForegroundLeasePromotion {
    let promotion = fence(session_id).promotion;
    let guard = promotion.lock_owned().await;
    ForegroundLeasePromotion {
        session_id: session_id.to_owned(),
        guard: Some(guard),
    }
}

impl Drop for ActForegroundSlot {
    fn drop(&mut self) {
        drop(self.exclusive.take());
        prune(&self.session_id);
    }
}

/// Shared hold on a session's fence; keep it alive until the operation ends.
pub(crate) struct SessionOperationHold {
    session_id: String,
    shared: Option<OwnedRwLockReadGuard<()>>,
}

impl Drop for SessionOperationHold {
    fn drop(&mut self) {
        drop(self.shared.take());
        prune(&self.session_id);
    }
}

/// Admits a standalone lease/profile/act operation by `session_id`, refusing
/// it while that session's own foreground escalation is in flight.
pub(crate) fn admit_session_operation(
    session_id: &str,
    tool: &str,
) -> Result<SessionOperationHold, ErrorData> {
    let operation = fence(session_id).operation;
    let shared = operation.try_read_owned().map_err(|_escalating| {
        prune(session_id);
        busy(&format!(
            "{tool} refused: this session's act operation=foreground escalation is in flight and owns its lease/profile until it unwinds; retry after it completes"
        ))
    })?;
    Ok(SessionOperationHold {
        session_id: session_id.to_owned(),
        shared: Some(shared),
    })
}

/// Waits for `session_id`'s in-flight escalation, if any, to finish its
/// unwind. Session teardown calls this before reclaiming the session's lease
/// and inputs, so it never pulls them out from under a running escalation.
/// Must be awaited before taking the lease transition gate, which the unwind
/// itself needs. An escalation that claims the slot afterwards refuses a
/// terminated session or draining daemon itself.
pub(crate) async fn await_session_escalation(session_id: &str) {
    let settled = fence(session_id).operation.read_owned().await;
    drop(settled);
    prune(session_id);
}

fn busy(message: &str) -> ErrorData {
    mcp_error(
        error_codes::ACTION_FOREGROUND_LEASE_BUSY,
        message.to_owned(),
    )
}

fn fences() -> std::sync::MutexGuard<'static, HashMap<String, Fence>> {
    FENCES.lock().unwrap_or_else(PoisonError::into_inner)
}

fn fence(session_id: &str) -> Fence {
    fences().entry(session_id.to_owned()).or_default().clone()
}

/// Drops a session's fence once nothing references it but the map.
fn prune(session_id: &str) {
    let mut map = fences();
    if map.get(session_id).is_some_and(|fence| {
        Arc::strong_count(&fence.operation) == 1 && Arc::strong_count(&fence.promotion) == 1
    }) {
        map.remove(session_id);
    }
}

impl super::SynapseService {
    /// Whether session teardown has marked `session_id` terminated, or the
    /// daemon is draining for shutdown.
    pub(crate) fn session_closing(&self, session_id: &str) -> bool {
        self.drain_state_handle().snapshot().draining
            || self
                .terminated_sessions
                .lock()
                .is_ok_and(|terminated| terminated.contains(session_id))
    }
}

#[cfg(test)]
pub(crate) fn fence_count() -> usize {
    fences().len()
}

#[cfg(test)]
#[path = "tests/act_foreground_slot.rs"]
mod act_foreground_slot_tests;
