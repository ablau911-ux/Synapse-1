use synapse_core::{Action, ComboInput, Point};

use crate::{ActionBackend, ActionError, EmitState, recovery};

mod delivery_guard;
mod input;

#[cfg(test)]
#[path = "software/tests/delivery_guard_294.rs"]
mod delivery_guard_294;
mod keyboard;
mod mouse;
mod text;
mod utils;

#[derive(Debug, Default)]
pub struct SoftwareBackend;

impl SoftwareBackend {
    #[must_use]
    #[tracing::instrument(fields(backend = "software"))]
    pub fn new() -> Self {
        Self
    }
}

/// Reads the current software cursor position in Win32 screen coordinates.
///
/// # Errors
///
/// Returns `ActionError::BackendUnavailable` when the OS cursor location cannot
/// be read from the active input desktop.
pub fn cursor_position() -> Result<Point, ActionError> {
    mouse::cursor_position()
}

/// Moves the software cursor to a Win32 screen coordinate and returns the
/// separately-read final cursor position. Its one caller restores the human's
/// cursor after a foreground action, so it runs in a release scope (#294):
/// bare cursor moves pass, any key, button or wheel input is still refused.
///
/// # Errors
///
/// Returns `ActionError::BackendUnavailable` when the OS rejects the cursor
/// move or final readback does not match the requested point.
pub fn set_cursor_position(point: Point) -> Result<Point, ActionError> {
    let _release_scope =
        delivery_guard::enter_delivery_scope(None, delivery_guard::InputKind::ReleaseOnly);
    mouse::set_cursor_position(point)
}

/// Checks the live foreground before dispatching a wheel event to a bound window.
///
/// # Errors
///
/// Returns `ACTION_FOREGROUND_LOST` if the live root window or process differs.
pub fn ensure_pointer_foreground(target: &crate::InputDeliveryTarget) -> Result<(), ActionError> {
    delivery_guard::check_delivery(
        Some(delivery_guard::DeliveryTargetId {
            root_hwnd: target.root_hwnd,
            pid: target.pid,
        }),
        delivery_guard::foreground_identity(),
        delivery_guard::InputKind::Pointer,
    )
}

impl ActionBackend for SoftwareBackend {
    #[tracing::instrument(skip_all, fields(backend = "software"))]
    fn execute(&self, action: &Action, state: &mut EmitState) -> Result<(), ActionError> {
        crate::validate_action(action)?;
        let stored_automation = state.delivery_target().is_some_and(|target| {
            target.source == crate::delivery_target::STORED_AUTOMATION_TARGET_SOURCE
        });
        let target = state
            .delivery_target()
            .map(|target| delivery_guard::DeliveryTargetId {
                root_hwnd: target.root_hwnd,
                pid: target.pid,
            });
        let kind = if matches!(action, Action::ReleaseAll) {
            delivery_guard::InputKind::ReleaseOnly
        } else {
            delivery_guard::InputKind::Keyboard
        };
        let _delivery_scope = delivery_guard::enter_delivery_scope(target, kind)
            .with_pointer_target_check(stored_automation);
        match action {
            Action::KeyPress { key, hold_ms, .. } => keyboard::press_key(key, *hold_ms, state),
            Action::KeyDown { key, .. } => keyboard::key_down(key, state),
            Action::KeyUp { key, .. } => keyboard::key_up(key, state),
            Action::KeyChord { keys, hold_ms, .. } => keyboard::key_chord(keys, *hold_ms, state),
            Action::TypeText { text, dynamics, .. } => {
                text::type_text(text, dynamics, state.enqueued_release_epoch())
            }
            Action::MouseMove {
                to,
                curve,
                duration_ms,
                ..
            } => mouse::mouse_move(to, curve, *duration_ms),
            Action::MouseMoveRelative { dx, dy, .. } => mouse::mouse_move_relative(*dx, *dy),
            Action::MouseButton {
                button,
                action,
                hold_ms,
                ..
            } => mouse::mouse_button(*button, *action, *hold_ms, state),
            Action::MouseDrag {
                from,
                to,
                button,
                curve,
                duration_ms,
                ..
            } => mouse::mouse_drag(*from, *to, *button, curve, *duration_ms, state),
            Action::MouseStroke {
                path,
                button,
                profile,
                timing,
                motion_model,
                humanize,
                ..
            } => mouse::mouse_stroke(
                path,
                *button,
                *profile,
                timing,
                *motion_model,
                *humanize,
                state,
            ),
            Action::MouseScroll { dy, dx, at, .. } => mouse::mouse_scroll(*dy, *dx, *at),
            Action::AimAt { target, style, .. } => mouse::aim_at(target, *style),
            Action::Combo { steps, .. } => combo(steps, state),
            Action::ReleaseAll => release_all(state),
            Action::PadButton { .. }
            | Action::PadStick { .. }
            | Action::PadTrigger { .. }
            | Action::PadReport { .. } => Err(ActionError::BackendUnavailable {
                detail: "software backend cannot emit gamepad actions".to_owned(),
            }),
        }
    }
}

#[tracing::instrument(skip_all, fields(action_kind = "software_combo"))]
fn combo(steps: &[synapse_core::ComboStep], state: &mut EmitState) -> Result<(), ActionError> {
    let timeline = utils::InputTimeline::new();
    for step in steps {
        timeline.wait_for_offset(u64::from(step.at_ms))?;
        match &step.input {
            ComboInput::KeyDown { key } => keyboard::key_down(key, state)?,
            ComboInput::KeyUp { key } => keyboard::key_up(key, state)?,
            ComboInput::KeyPress { key, hold_ms } => {
                keyboard::press_key(key, u32::from(*hold_ms), state)?;
            }
            ComboInput::MouseButton { button, action } => {
                mouse::mouse_button(*button, *action, 0, state)?;
            }
            ComboInput::MouseMoveRel { dx, dy } => mouse::mouse_move_relative(*dx, *dy)?,
            ComboInput::PadButton { .. } | ComboInput::PadStick { .. } => {
                return Err(ActionError::BackendUnavailable {
                    detail: "software backend cannot emit gamepad combo steps".to_owned(),
                });
            }
        }
    }
    Ok(())
}

#[tracing::instrument(skip_all, fields(action_kind = "software_release_all"))]
fn release_all(state: &mut EmitState) -> Result<(), ActionError> {
    let snapshot = state.snapshot();
    let mut enigo = utils::enigo()?;
    keyboard::release_keys_with(&mut enigo, &snapshot.held_keys)?;
    mouse::release_buttons_with(&mut enigo, &snapshot.held_buttons)?;
    for key in &snapshot.held_keys {
        recovery::clear_held_key(key)?;
    }
    for button in &snapshot.held_buttons {
        recovery::clear_held_button(*button)?;
    }
    state.release_all();
    Ok(())
}
