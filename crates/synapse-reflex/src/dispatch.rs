use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use synapse_action::{
    ActionError, ActionHandle, InputDeliveryTarget, delivery_target::require_stored_input_target,
};
use synapse_core::{
    Action, ReflexId, ReflexState, SCHEMA_VERSION, StoredAuditContext, StoredReflexAudit,
    StoredReflexStep, error_codes, types::StoredInputTarget,
};
use synapse_storage::Db;
use uuid::Uuid;

use crate::{ReflexError, ReflexResult, write_audit};

pub const REFLEX_ACTION_PERMISSION_DENIED_KIND: &str = "reflex_action_permission_denied";
pub const REFLEX_ACTION_DENIED_STEP_STATUS: &str = "action_denied";
pub const REFLEX_INPUT_TARGET_REFUSED_KIND: &str = "reflex_input_target_refused";

pub type ReflexActionGateHandle = Arc<dyn ReflexActionGate>;

pub trait ReflexActionGate: Send + Sync {
    /// Checks whether a reflex action may dispatch at the current point in time.
    ///
    /// # Errors
    ///
    /// Returns [`ReflexActionPermissionDenied`] when the active policy refuses
    /// the action.
    fn ensure_action_allowed(
        &self,
        reflex_id: &ReflexId,
        action: &Action,
    ) -> Result<(), ReflexActionPermissionDenied>;

    /// Resolves a stored automation target against its live claim and foreground.
    /// The default has no live claim table, so it refuses every stored target.
    ///
    /// # Errors
    ///
    /// Returns the `ACTION_INPUT_TARGET_*` refusal from
    /// [`require_stored_input_target`].
    fn resolve_input_target(
        &self,
        reflex_id: &ReflexId,
        target: Option<StoredInputTarget>,
        _owner: Option<&str>,
    ) -> Result<InputDeliveryTarget, ActionError> {
        require_stored_input_target(reflex_id, target, None).map(InputDeliveryTarget::from_stored)
    }

    /// The foreground typing guard every reflex/routine `TypeText` must go
    /// through (#188). `None` (the default) means no guard is installed, and
    /// reflex `TypeText` is then refused before any key.
    fn foreground_type_guard(&self) -> Option<ReflexForegroundTypeGuardHandle> {
        None
    }
}

pub type ReflexForegroundTypeGuardHandle = Arc<dyn ReflexForegroundTypeGuard>;

/// Protects reflex/routine `TypeText` the way #188 protects `act_type`: the
/// text is typed only while the routine's target window is the foreground.
pub trait ReflexForegroundTypeGuard: Send + Sync {
    /// Enqueues the `TypeText` `action` on `handle` only when the target window
    /// is the foreground, then watches the foreground until the action
    /// finishes and raises the release interrupt on every departure. It must
    /// enqueue before returning, so the reflex's action order is kept.
    ///
    /// # Errors
    ///
    /// Returns `ActionError::ForegroundLost` when the target is not the
    /// foreground or the foreground cannot be read (no input was sent), or the
    /// enqueue error.
    fn dispatch_type_text(
        &self,
        reflex_id: &ReflexId,
        action: Action,
        handle: &ActionHandle,
    ) -> Result<(), ActionError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReflexActionPermissionDenied {
    pub policy_code: Option<String>,
    pub policy_reason: Option<String>,
    pub profile_id: Option<String>,
    pub use_scope: Option<String>,
    pub detail: String,
}

impl ReflexActionPermissionDenied {
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            policy_code: None,
            policy_reason: None,
            profile_id: None,
            use_scope: None,
            detail: detail.into(),
        }
    }
}

#[derive(Clone)]
pub struct ReflexActionDispatchContext {
    action_handle: ActionHandle,
    action_gate: Option<ReflexActionGateHandle>,
    audit_db: Option<Arc<Db>>,
    audit_context: Option<StoredAuditContext>,
    tick_index: u64,
    input_target: Option<StoredInputTarget>,
    input_owner: Option<String>,
}

impl ReflexActionDispatchContext {
    #[must_use]
    pub fn new(
        action_handle: ActionHandle,
        action_gate: Option<ReflexActionGateHandle>,
        audit_db: Option<Arc<Db>>,
        audit_context: Option<StoredAuditContext>,
        tick_index: u64,
    ) -> Self {
        Self {
            action_handle,
            action_gate,
            audit_db,
            audit_context,
            tick_index,
            input_target: None,
            input_owner: None,
        }
    }

    pub(crate) fn with_reflex_target(mut self, reflex: &crate::ScheduledReflex) -> Self {
        self.input_target = reflex.input_target;
        self.input_owner.clone_from(&reflex.input_owner);
        self
    }

    /// Dispatches one reflex action. OS input (everything but releases and
    /// virtual-pad reports) is delivered only to the reflex's stored target
    /// window, proven claimed and in front at this moment (#312).
    pub fn dispatch_action(&self, reflex_id: &ReflexId, action: &Action) -> ReflexResult<()> {
        self.ensure_action_allowed(reflex_id, action)?;
        let handle = self.action_handle.with_session_id(self.input_owner.clone());
        let handle = if requires_input_target(action) {
            let delivery = self
                .resolve_input_target(reflex_id)
                .map_err(|error| ReflexError::InputTarget { error })?;
            handle.with_delivery_target(Some(delivery))
        } else {
            handle
        };
        if matches!(action, Action::TypeText { .. }) {
            return self.dispatch_foreground_type_text(reflex_id, action, &handle);
        }

        handle.try_execute(action.clone()).map_err(dispatch_failed)
    }

    /// Live proof comes from the installed gate; without one nothing can be
    /// proven claimed and in front, so the stored target is refused.
    fn resolve_input_target(
        &self,
        reflex_id: &ReflexId,
    ) -> Result<InputDeliveryTarget, ActionError> {
        self.action_gate.as_ref().map_or_else(
            || {
                require_stored_input_target(reflex_id, self.input_target, None)
                    .map(InputDeliveryTarget::from_stored)
            },
            |gate| {
                gate.resolve_input_target(reflex_id, self.input_target, self.input_owner.as_deref())
            },
        )
    }

    /// Keep the #188 monitor active while the stored window receives text.
    fn dispatch_foreground_type_text(
        &self,
        reflex_id: &ReflexId,
        action: &Action,
        handle: &ActionHandle,
    ) -> ReflexResult<()> {
        let guard = self
            .action_gate
            .as_ref()
            .and_then(|gate| gate.foreground_type_guard());
        let Some(guard) = guard else {
            return Err(foreground_lost(
                reflex_id,
                "reflex TypeText refused before the first key: no foreground typing guard is installed; no input was sent".to_owned(),
            ));
        };
        guard
            .dispatch_type_text(reflex_id, action.clone(), handle)
            .map_err(|error| match error {
                ActionError::ForegroundLost { detail } => foreground_lost(reflex_id, detail),
                other => dispatch_failed(other),
            })
    }

    pub fn ensure_action_allowed(&self, reflex_id: &ReflexId, action: &Action) -> ReflexResult<()> {
        if let Some(gate) = &self.action_gate
            && let Err(denial) = gate.ensure_action_allowed(reflex_id, action)
        {
            self.write_action_denied_audit(reflex_id, action, &denial);
            return Err(ReflexError::ActionPermissionDenied {
                reflex_id: reflex_id.clone(),
                detail: denial.detail,
            });
        }
        Ok(())
    }

    fn write_action_denied_audit(
        &self,
        reflex_id: &ReflexId,
        action: &Action,
        denial: &ReflexActionPermissionDenied,
    ) {
        let Some(db) = self.audit_db.as_deref() else {
            return;
        };
        let audit = StoredReflexAudit {
            schema_version: SCHEMA_VERSION,
            audit_id: Uuid::now_v7().to_string(),
            reflex_id: reflex_id.clone(),
            ts_ns: now_ts_ns(),
            status: ReflexState::ActionDenied,
            event_id: None,
            audit_context: self.audit_context.clone(),
            steps: vec![StoredReflexStep {
                index: 0,
                action: action.clone(),
                status: REFLEX_ACTION_DENIED_STEP_STATUS.to_owned(),
                error_code: Some(error_codes::REFLEX_ACTION_PERMISSION_DENIED.to_owned()),
            }],
            error_code: Some(error_codes::REFLEX_ACTION_PERMISSION_DENIED.to_owned()),
            details: json!({
                "kind": REFLEX_ACTION_PERMISSION_DENIED_KIND,
                "reason": error_codes::REFLEX_ACTION_PERMISSION_DENIED,
                "policy_code": denial.policy_code,
                "policy_reason": denial.policy_reason,
                "profile_id": denial.profile_id,
                "use_scope": denial.use_scope,
                "detail": denial.detail,
                "action_kind": action_kind(action),
                "tick_index": self.tick_index,
            }),
            redacted: false,
            redactions: Vec::new(),
        };

        if let Err(error) = write_audit(db, &audit).and_then(|()| db.flush()) {
            tracing::warn!(
                component = "reflex_dispatch",
                reflex_id = %audit.reflex_id,
                audit_id = %audit.audit_id,
                detail = %error,
                "reflex action-denied audit write failed"
            );
        }
    }
}

const fn requires_input_target(action: &Action) -> bool {
    // Recovery key/button ups are intentionally target-free in the emitter.
    !matches!(
        action,
        Action::ReleaseAll
            | Action::KeyUp { .. }
            | Action::MouseButton {
                action: synapse_core::ButtonAction::Up,
                ..
            }
            | Action::PadButton { .. }
            | Action::PadStick { .. }
            | Action::PadTrigger { .. }
            | Action::PadReport { .. }
    )
}

fn dispatch_failed(error: ActionError) -> ReflexError {
    ReflexError::ParamsInvalid {
        detail: format!("scheduler action dispatch failed: {error}"),
    }
}

fn foreground_lost(reflex_id: &ReflexId, detail: String) -> ReflexError {
    tracing::warn!(
        code = error_codes::ACTION_FOREGROUND_LOST,
        reflex_id = %reflex_id,
        detail = %detail,
        "reflex TypeText refused: target window is not the foreground"
    );
    ReflexError::ForegroundLost {
        reflex_id: reflex_id.clone(),
        detail,
    }
}

const fn action_kind(action: &Action) -> &'static str {
    match action {
        Action::KeyPress { .. } => "key_press",
        Action::KeyDown { .. } => "key_down",
        Action::KeyUp { .. } => "key_up",
        Action::KeyChord { .. } => "key_chord",
        Action::TypeText { .. } => "type_text",
        Action::MouseMove { .. } => "mouse_move",
        Action::MouseMoveRelative { .. } => "mouse_move_relative",
        Action::MouseButton { .. } => "mouse_button",
        Action::MouseDrag { .. } => "mouse_drag",
        Action::MouseStroke { .. } => "mouse_stroke",
        Action::MouseScroll { .. } => "mouse_scroll",
        Action::PadButton { .. } => "pad_button",
        Action::PadStick { .. } => "pad_stick",
        Action::PadTrigger { .. } => "pad_trigger",
        Action::PadReport { .. } => "pad_report",
        Action::AimAt { .. } => "aim_at",
        Action::Combo { .. } => "combo",
        Action::ReleaseAll => "release_all",
    }
}

fn now_ts_ns() -> u64 {
    Utc::now()
        .timestamp_nanos_opt()
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/dispatch_foreground_type_188.rs"]
mod foreground_type_tests;
