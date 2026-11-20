//! #312 (a0a9d7ad): reflex OS input goes only to the reflex's stored target
//! window, proven claimed and in front by the installed action gate at the
//! moment of delivery. A test that expects its reflex actions to run stores
//! [`TEST_TARGET`] on the reflex and installs [`claimed_target_gate`].

use std::sync::Arc;

use synapse_action::{
    ActionError, InputDeliveryTarget, delivery_target::require_stored_input_target,
};
use synapse_core::{Action, ReflexId, types::StoredInputTarget};
use synapse_reflex::{
    ReflexActionGate, ReflexActionGateHandle, ReflexActionPermissionDenied, ScheduledReflex,
};

/// The window every test reflex stores. No real window is behind it: the
/// actions only reach the test's action channel.
pub const TEST_TARGET: StoredInputTarget = StoredInputTarget {
    root_hwnd: 0x312,
    pid: 312,
};

/// Stand-in for the daemon's gate: allows every action and proves exactly one
/// window claimed and in front, so any other stored window is still refused.
struct ClaimedForegroundGate {
    claimed: StoredInputTarget,
}

impl ReflexActionGate for ClaimedForegroundGate {
    fn ensure_action_allowed(
        &self,
        _reflex_id: &ReflexId,
        _action: &Action,
    ) -> Result<(), ReflexActionPermissionDenied> {
        Ok(())
    }

    fn resolve_input_target(
        &self,
        reflex_id: &ReflexId,
        target: Option<StoredInputTarget>,
        _owner: Option<&str>,
    ) -> Result<InputDeliveryTarget, ActionError> {
        require_stored_input_target(reflex_id, target, Some(self.claimed))
            .map(InputDeliveryTarget::from_stored)
    }
}

/// A gate that proves [`TEST_TARGET`] claimed and in front.
pub fn claimed_target_gate() -> ReflexActionGateHandle {
    Arc::new(ClaimedForegroundGate {
        claimed: TEST_TARGET,
    })
}

/// The reflex with [`TEST_TARGET`] stored as its OS-input window.
pub fn targeted(reflex: ScheduledReflex) -> ScheduledReflex {
    reflex.with_input_target(Some(TEST_TARGET), None)
}
