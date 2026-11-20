//! Per-batch OS-input delivery policy. Release-only batches always pass.

use std::cell::Cell;

use crate::ActionError;
use crate::delivery_target::INPUT_TARGET_REMEDIATION;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeliveryTargetId {
    pub root_hwnd: i64,
    pub pid: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputKind {
    Keyboard,
    Pointer,
    ReleaseOnly,
}

pub(crate) fn check_delivery(
    target: Option<DeliveryTargetId>,
    foreground: Option<DeliveryTargetId>,
    kind: InputKind,
) -> Result<(), ActionError> {
    if kind == InputKind::ReleaseOnly {
        return Ok(());
    }
    let target = target.ok_or_else(|| ActionError::InputTargetRequired {
        detail: format!(
            "{kind:?} OS input refused before delivery: the action has no intended target window (root HWND + pid), so nothing is sent to whatever window is foreground. {INPUT_TARGET_REMEDIATION}"
        ),
    })?;
    if foreground != Some(target) {
        return Err(ActionError::ForegroundLost {
            detail: format!(
                "{kind:?} OS input refused before this batch: target={target:?} is not the live foreground (foreground={foreground:?}); the batch was not sent"
            ),
        });
    }
    Ok(())
}

type ForegroundProbe = fn() -> Option<DeliveryTargetId>;

thread_local! {
    static DELIVERY_SCOPE: Cell<(Option<DeliveryTargetId>, InputKind)> =
        const { Cell::new((None, InputKind::Keyboard)) };
    static CHECK_POINTER_TARGET: Cell<bool> = const { Cell::new(false) };
    static FOREGROUND_PROBE: Cell<ForegroundProbe> = const { Cell::new(live_foreground) };
}

fn live_foreground() -> Option<DeliveryTargetId> {
    synapse_a11y::foreground_root_identity()
        .ok()
        .map(|identity| DeliveryTargetId {
            root_hwnd: identity.root_hwnd,
            pid: identity.pid,
        })
}

/// Re-reads the foreground at each physical delivery boundary.
pub(super) fn ensure_delivery(kind: InputKind) -> Result<(), ActionError> {
    let (target, _) = delivery_scope();
    if kind == InputKind::ReleaseOnly {
        return check_delivery(target, None, kind);
    }
    check_delivery(target, foreground_identity(), kind)
}

/// A bare cursor move (no button, key or wheel). Inside a release scope
/// (`ReleaseAll`, the operator's cursor-context restore) it is part of putting
/// the human's state back and always passes; elsewhere it is pointer input.
pub(super) fn ensure_cursor_move() -> Result<(), ActionError> {
    if delivery_scope().1 == InputKind::ReleaseOnly {
        return Ok(());
    }
    ensure_delivery(InputKind::Pointer)
}

/// Classifies actual records, so cleanup key/button ups remain available even
/// after focus changes. Unknown record types fail closed as keyboard input.
pub(super) fn ensure_send_input_batch(
    inputs: &[windows::Win32::UI::Input::KeyboardAndMouse::INPUT],
) -> Result<(), ActionError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT_KEYBOARD, INPUT_MOUSE, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
        MOUSEEVENTF_XUP,
    };
    let releases =
        MOUSEEVENTF_LEFTUP | MOUSEEVENTF_RIGHTUP | MOUSEEVENTF_MIDDLEUP | MOUSEEVENTF_XUP;
    let moves = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    let mut kind = InputKind::ReleaseOnly;
    let mut pointer_press = false;
    for input in inputs {
        if input.r#type == INPUT_KEYBOARD {
            // SAFETY: the input tag selects the keyboard union arm.
            if !unsafe { input.Anonymous.ki.dwFlags }.contains(KEYEVENTF_KEYUP) {
                kind = InputKind::Keyboard;
                break;
            }
        } else if input.r#type == INPUT_MOUSE {
            // SAFETY: the input tag selects the mouse union arm.
            let flags = unsafe { input.Anonymous.mi.dwFlags };
            let delivering = flags.0 & !releases.0;
            if delivering != 0 {
                kind = InputKind::Pointer;
                pointer_press |= delivering & !moves.0 != 0;
            }
        } else {
            kind = InputKind::Keyboard;
            break;
        }
    }
    if kind == InputKind::Pointer && !pointer_press {
        return ensure_cursor_move();
    }
    ensure_delivery(kind)?;
    if kind == InputKind::Pointer && CHECK_POINTER_TARGET.with(Cell::get) {
        ensure_pointer_recipient()?;
    }
    Ok(())
}

fn ensure_pointer_recipient() -> Result<(), ActionError> {
    use windows::Win32::{Foundation::POINT, UI::WindowsAndMessaging::WindowFromPoint};

    let cursor = super::mouse::cursor_position()?;
    // SAFETY: WindowFromPoint accepts a screen point by value.
    let hwnd = unsafe {
        WindowFromPoint(POINT {
            x: cursor.x,
            y: cursor.y,
        })
    };
    let recipient = synapse_a11y::window_root_identity(hwnd.0 as isize as i64)
        .ok()
        .map(|identity| DeliveryTargetId {
            root_hwnd: identity.root_hwnd,
            pid: identity.pid,
        });
    let (target, _) = delivery_scope();
    if recipient == target {
        return Ok(());
    }
    Err(ActionError::InputTargetNotReady {
        detail:
            "pointer input refused before delivery: the cursor is outside the stored target window"
                .to_owned(),
    })
}

pub(crate) fn foreground_identity() -> Option<DeliveryTargetId> {
    FOREGROUND_PROBE.with(|probe| probe.get()())
}

pub(crate) fn delivery_scope() -> (Option<DeliveryTargetId>, InputKind) {
    DELIVERY_SCOPE.with(Cell::get)
}

/// Restores the caller's scope on normal return and panic.
pub(crate) struct DeliveryScope {
    previous: (Option<DeliveryTargetId>, InputKind),
    previous_check_cursor: bool,
}

impl DeliveryScope {
    pub(super) fn with_pointer_target_check(self, enabled: bool) -> Self {
        CHECK_POINTER_TARGET.with(|check| check.set(enabled));
        self
    }
}

pub(crate) fn enter_delivery_scope(
    target: Option<DeliveryTargetId>,
    kind: InputKind,
) -> DeliveryScope {
    DeliveryScope {
        previous: DELIVERY_SCOPE.with(|scope| scope.replace((target, kind))),
        previous_check_cursor: CHECK_POINTER_TARGET.with(|check| check.replace(false)),
    }
}

impl Drop for DeliveryScope {
    fn drop(&mut self) {
        DELIVERY_SCOPE.with(|scope| scope.set(self.previous));
        CHECK_POINTER_TARGET.with(|check| check.set(self.previous_check_cursor));
    }
}

#[cfg(test)]
pub(super) struct TestForegroundProbe {
    previous: ForegroundProbe,
}

#[cfg(test)]
pub(super) fn install_foreground_probe(probe: ForegroundProbe) -> TestForegroundProbe {
    TestForegroundProbe {
        previous: FOREGROUND_PROBE.with(|slot| slot.replace(probe)),
    }
}

#[cfg(test)]
impl Drop for TestForegroundProbe {
    fn drop(&mut self) {
        FOREGROUND_PROBE.with(|slot| slot.set(self.previous));
    }
}
