//! #188 follow-up (A): reflex/routine `TypeText` sends keys into whatever
//! window is in front, so it must only go through the installed foreground
//! typing guard: refused with `ACTION_FOREGROUND_LOST` (no key enqueued) when
//! the target is not the foreground or no guard exists, typed exactly once
//! when the target is the foreground. Other reflex actions are unaffected.

use std::sync::{Arc, Mutex};

use synapse_action::{
    ActionError, ActionHandle, ActionMessage, InputDeliveryTarget,
    delivery_target::require_stored_input_target,
};
use synapse_core::{
    Action, Backend, Key, KeyCode, KeystrokeDynamics, ReflexId, error_codes,
    types::StoredInputTarget,
};
use tokio::sync::mpsc;

use super::{
    ReflexActionDispatchContext, ReflexActionGate, ReflexActionPermissionDenied,
    ReflexForegroundTypeGuard, ReflexForegroundTypeGuardHandle,
};
use crate::ScheduledReflex;

/// #312: the routine's stored OS-input window. No real window is behind it.
const ROUTINE_WINDOW: StoredInputTarget = StoredInputTarget {
    root_hwnd: 0x188,
    pid: 188,
};

/// Stand-in for the daemon's guard: the target is or is not the foreground.
struct FakeForegroundGuard {
    target_is_foreground: bool,
    calls: Mutex<usize>,
}

impl ReflexForegroundTypeGuard for FakeForegroundGuard {
    fn dispatch_type_text(
        &self,
        _reflex_id: &ReflexId,
        action: Action,
        handle: &ActionHandle,
    ) -> Result<(), ActionError> {
        if let Ok(mut calls) = self.calls.lock() {
            *calls += 1;
        }
        if !self.target_is_foreground {
            return Err(ActionError::ForegroundLost {
                detail: "target hwnd 0x1 is not the foreground; no input was sent".to_owned(),
            });
        }
        handle.try_execute(action)
    }
}

struct AllowAllGate {
    guard: Option<Arc<FakeForegroundGuard>>,
}

impl ReflexActionGate for AllowAllGate {
    fn ensure_action_allowed(
        &self,
        _reflex_id: &ReflexId,
        _action: &Action,
    ) -> Result<(), ReflexActionPermissionDenied> {
        Ok(())
    }

    /// #312: proves the routine's window claimed and in front, so the stored
    /// target check passes and the foreground typing guard decides.
    fn resolve_input_target(
        &self,
        reflex_id: &ReflexId,
        target: Option<StoredInputTarget>,
        _owner: Option<&str>,
    ) -> Result<InputDeliveryTarget, ActionError> {
        require_stored_input_target(reflex_id, target, Some(ROUTINE_WINDOW))
            .map(InputDeliveryTarget::from_stored)
    }

    fn foreground_type_guard(&self) -> Option<ReflexForegroundTypeGuardHandle> {
        self.guard
            .as_ref()
            .map(|guard| Arc::clone(guard) as ReflexForegroundTypeGuardHandle)
    }
}

fn guard(target_is_foreground: bool) -> Arc<FakeForegroundGuard> {
    Arc::new(FakeForegroundGuard {
        target_is_foreground,
        calls: Mutex::new(0),
    })
}

fn guard_calls(guard: &FakeForegroundGuard) -> usize {
    guard.calls.lock().map(|calls| *calls).unwrap_or_default()
}

/// A dispatch context for a routine that stores `ROUTINE_WINDOW` (#312).
fn context(
    guard: Option<Arc<FakeForegroundGuard>>,
    with_gate: bool,
) -> (ReflexActionDispatchContext, mpsc::Receiver<ActionMessage>) {
    let (context, rx) = unbound_context(guard, with_gate);
    let routine = ScheduledReflex::every_tick(reflex_id(), Vec::new())
        .with_input_target(Some(ROUTINE_WINDOW), None);
    (context.with_reflex_target(&routine), rx)
}

/// A dispatch context for a routine with no stored window.
fn unbound_context(
    guard: Option<Arc<FakeForegroundGuard>>,
    with_gate: bool,
) -> (ReflexActionDispatchContext, mpsc::Receiver<ActionMessage>) {
    let (handle, rx) = ActionHandle::channel();
    let gate = with_gate.then(|| Arc::new(AllowAllGate { guard }) as Arc<dyn ReflexActionGate>);
    (
        ReflexActionDispatchContext::new(handle, gate, None, None, 0),
        rx,
    )
}

fn reflex_id() -> ReflexId {
    ReflexId::from("routine-type-188")
}

fn type_text(text: &str) -> Action {
    Action::TypeText {
        text: text.to_owned(),
        dynamics: KeystrokeDynamics::Burst,
        backend: Backend::Software,
    }
}

fn assert_nothing_enqueued(rx: &mut mpsc::Receiver<ActionMessage>) {
    assert!(
        matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "no input may be enqueued"
    );
}

#[test]
fn must_fail_routine_type_text_refused_when_target_not_foreground() {
    let refusing = guard(false);
    let (context, mut rx) = context(Some(Arc::clone(&refusing)), true);
    let error = context
        .dispatch_action(&reflex_id(), &type_text("password"))
        .expect_err("TypeText must be refused when the target is not the foreground");
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    assert_eq!(guard_calls(&refusing), 1);
    assert_nothing_enqueued(&mut rx);
}

#[test]
fn must_fail_routine_type_text_refused_when_no_guard_is_installed() {
    let (context, mut rx) = context(None, true);
    let error = context
        .dispatch_action(&reflex_id(), &type_text("password"))
        .expect_err("TypeText without a foreground guard must be refused");
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    assert_nothing_enqueued(&mut rx);
}

#[test]
fn must_fail_routine_type_text_refused_without_any_action_gate() {
    let (context, mut rx) = context(None, false);
    let error = context
        .dispatch_action(&reflex_id(), &type_text("password"))
        .expect_err("TypeText without a gate (so without a guard) must be refused");
    // #312: with no gate nothing proves the stored window claimed and in
    // front, so the refusal comes before the typing guard.
    assert_eq!(error.code(), error_codes::ACTION_INPUT_TARGET_NOT_READY);
    assert_nothing_enqueued(&mut rx);
}

#[test]
fn must_fail_routine_type_text_without_stored_target_is_refused_312() {
    let accepting = guard(true);
    let (context, mut rx) = unbound_context(Some(Arc::clone(&accepting)), true);
    let error = context
        .dispatch_action(&reflex_id(), &type_text("password"))
        .expect_err("a routine with no stored window must not type anywhere");
    assert_eq!(error.code(), error_codes::ACTION_INPUT_TARGET_REQUIRED);
    assert_eq!(guard_calls(&accepting), 0);
    assert_nothing_enqueued(&mut rx);
}

#[test]
fn must_pass_routine_type_text_on_target_is_enqueued_exactly_once() {
    let accepting = guard(true);
    let (context, mut rx) = context(Some(Arc::clone(&accepting)), true);
    context
        .dispatch_action(&reflex_id(), &type_text("hello routine"))
        .expect("target is the foreground, TypeText goes through");
    assert_eq!(guard_calls(&accepting), 1);
    let (action, _ack) = rx.try_recv().expect("the TypeText must be enqueued");
    assert_eq!(action, type_text("hello routine"));
    assert_nothing_enqueued(&mut rx);
}

#[test]
fn must_pass_non_text_reflex_actions_bypass_the_foreground_guard() {
    let refusing = guard(false);
    let (context, mut rx) = context(Some(Arc::clone(&refusing)), true);
    let press = Action::KeyPress {
        key: Key {
            code: KeyCode::Symbol { value: 'a' },
            use_scancode: false,
        },
        hold_ms: 10,
        backend: Backend::Software,
    };
    context
        .dispatch_action(&reflex_id(), &press)
        .expect("key presses are not foreground-typing guarded");
    assert_eq!(guard_calls(&refusing), 0);
    let (action, _ack) = rx.try_recv().expect("the key press must be enqueued");
    assert_eq!(action, press);
}
