use std::{
    thread,
    time::{Duration, Instant},
};

use synapse_core::KeystrokeDynamics;
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            GUITHREADINFO, GetGUIThreadInfo, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW,
            WM_NULL,
        },
    },
};

use super::input::{keyboard_input, send_input_batch, virtual_keyboard_input};
#[cfg(not(test))]
use super::utils::sleep_ms;
use crate::ActionError;
use crate::backend::text_dispatch::{TextDispatchInput, TextDispatchStep, text_dispatch_plan};
use crate::hotkey::{operator_release_epoch, operator_release_requested_since};
#[cfg(test)]
use readiness_test_support::{FocusReadyClock, sleep_ms};
#[cfg(not(test))]
use std::time::Instant as FocusReadyClock;

const MAX_TYPE_TEXT_RUNTIME: Duration = Duration::from_secs(25);
const TYPE_TEXT_FOCUS_READY_TIMEOUT: Duration = Duration::from_millis(250);

#[tracing::instrument(skip_all, fields(action_kind = "software_type_text"))]
pub(super) fn type_text(
    text: &str,
    dynamics: &KeystrokeDynamics,
    enqueued_release_epoch: Option<u64>,
) -> Result<(), ActionError> {
    let epoch = enqueued_release_epoch.unwrap_or_else(operator_release_epoch);
    if !text.is_empty() {
        ensure_type_text_not_interrupted(epoch, 0)?;
        wait_for_focus_ready(epoch)?;
    }
    run_type_steps(
        text_dispatch_plan(text, dynamics),
        epoch,
        sleep_ms,
        send_text_input,
    )
}

/// Drives a typing plan, stopping before the next key once the operator hotkey
/// or `release_all` raised the release interrupt (#188). Interrupts are measured
/// against `epoch`, the release epoch read when the action was ENQUEUED, not
/// when this run starts: an interrupt raised after enqueue but before the first
/// key (for example by the foreground monitor while the action still waited in
/// the queue) stops the run before that key. Every check reuses the same epoch,
/// so an interrupt landing between delays or while a key is being sent is seen
/// on the next check, not only inside the delay sleep.
fn run_type_steps(
    steps: Vec<TextDispatchStep>,
    epoch: u64,
    mut sleep: impl FnMut(u32) -> bool,
    mut send: impl FnMut(TextDispatchInput) -> Result<(), ActionError>,
) -> Result<(), ActionError> {
    let started = Instant::now();
    let mut sent = 0_usize;
    for step in steps {
        ensure_type_text_runtime(started, Instant::now())?;
        ensure_type_text_not_interrupted(epoch, sent)?;
        if sleep(step.iki_ms_before) {
            return Err(type_text_interrupted(sent));
        }
        ensure_type_text_runtime(started, Instant::now())?;
        for input in step.inputs {
            ensure_type_text_not_interrupted(epoch, sent)?;
            send(input)?;
            sent += 1;
            thread::yield_now();
        }
        thread::yield_now();
    }
    Ok(())
}

fn wait_for_focus_ready(release_epoch: u64) -> Result<(), ActionError> {
    let deadline = FocusReadyClock::now() + TYPE_TEXT_FOCUS_READY_TIMEOUT;
    loop {
        ensure_type_text_not_interrupted(release_epoch, 0)?;
        let remaining = deadline.saturating_duration_since(FocusReadyClock::now());
        let Ok(timeout_ms) = u32::try_from(remaining.as_millis()) else {
            return Err(focus_ready_timeout());
        };
        if timeout_ms == 0 {
            return Err(focus_ready_timeout());
        }
        let ready = focused_queue_ready(timeout_ms)?;
        let now = FocusReadyClock::now();
        if now > deadline {
            return Err(focus_ready_timeout());
        }
        if ready {
            return Ok(());
        }
        if deadline.saturating_duration_since(now) < Duration::from_millis(1) {
            return Err(focus_ready_timeout());
        }
        if sleep_ms(1) {
            return Err(type_text_interrupted(0));
        }
    }
}

fn focus_ready_timeout() -> ActionError {
    ActionError::InputTargetNotReady {
        detail: "focused field did not become input-ready within 250 ms; 0 input units sent"
            .to_owned(),
    }
}

fn focused_queue_ready(timeout_ms: u32) -> Result<bool, ActionError> {
    #[cfg(test)]
    if let Some(ready) = readiness_test_support::probe(timeout_ms) {
        return Ok(ready);
    }
    let mut before = GUITHREADINFO {
        cbSize: u32::try_from(std::mem::size_of::<GUITHREADINFO>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    // SAFETY: `before` is writable, initialized storage with the required cbSize;
    // thread id zero queries the current foreground thread.
    unsafe { GetGUIThreadInfo(0, &raw mut before) }.map_err(|error| {
        ActionError::BackendUnavailable {
            detail: format!("cannot confirm native keyboard focus: {error}"),
        }
    })?;
    if before.hwndFocus.0.is_null() {
        return Ok(false);
    }
    // A round trip to the focused control confirms its thread serviced pending
    // activation/focus messages. A ready field returns immediately; no fixed sleep.
    // SAFETY: hwndFocus came from GetGUIThreadInfo; WM_NULL carries no pointers,
    // and the bounded call safely reports failure if the window was destroyed.
    let ready = unsafe {
        SendMessageTimeoutW(
            before.hwndFocus,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            timeout_ms,
            None,
        )
    };
    if ready.0 == 0 {
        return Ok(false);
    }
    let mut after = GUITHREADINFO {
        cbSize: before.cbSize,
        ..Default::default()
    };
    // SAFETY: `after` is writable, initialized storage with the required cbSize;
    // no pointers from the earlier snapshot are dereferenced.
    unsafe { GetGUIThreadInfo(0, &raw mut after) }.map_err(|error| {
        ActionError::BackendUnavailable {
            detail: format!("cannot read back native keyboard focus: {error}"),
        }
    })?;
    Ok(before.hwndFocus == after.hwndFocus && before.hwndActive == after.hwndActive)
}

fn ensure_type_text_not_interrupted(epoch: u64, sent: usize) -> Result<(), ActionError> {
    if operator_release_requested_since(epoch) {
        return Err(type_text_interrupted(sent));
    }
    Ok(())
}

fn type_text_interrupted(sent: usize) -> ActionError {
    ActionError::SafetyOperatorHotkeyFired {
        detail: format!(
            "operator release requested during foreground text dispatch after {sent} input unit(s); remaining text was not sent"
        ),
    }
}

fn ensure_type_text_runtime(started: Instant, now: Instant) -> Result<(), ActionError> {
    if now.saturating_duration_since(started) > MAX_TYPE_TEXT_RUNTIME {
        return Err(ActionError::BackendUnavailable {
            detail: format!(
                "foreground text dispatch exceeded the {} ms runtime watchdog and was stopped",
                MAX_TYPE_TEXT_RUNTIME.as_millis()
            ),
        });
    }
    Ok(())
}

fn send_text_input(input: TextDispatchInput) -> Result<(), ActionError> {
    #[cfg(test)]
    if readiness_test_support::capture(input) {
        return Ok(());
    }
    match input {
        TextDispatchInput::UnicodeUnit(unit) => send_unicode_unit(unit),
        TextDispatchInput::VirtualKey(vkey) => {
            send_virtual_key(VIRTUAL_KEY(vkey), "text virtual key")
        }
    }
}

fn send_unicode_unit(unit: u16) -> Result<(), ActionError> {
    let inputs = [
        keyboard_input(unit, KEYEVENTF_UNICODE),
        keyboard_input(unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
    ];
    send_input_batch(&inputs, "unicode text character")
}

fn send_virtual_key(vkey: VIRTUAL_KEY, detail: &'static str) -> Result<(), ActionError> {
    let inputs = [
        virtual_keyboard_input(vkey, KEYBD_EVENT_FLAGS(0)),
        virtual_keyboard_input(vkey, KEYEVENTF_KEYUP),
    ];
    send_input_batch(&inputs, detail)
}

#[cfg(test)]
#[path = "tests/text_interrupt_188.rs"]
mod interrupt_tests;

#[cfg(test)]
#[path = "tests/text_enqueue_epoch_188.rs"]
mod enqueue_epoch_tests;

#[cfg(test)]
#[path = "tests/text_readiness_support_281.rs"]
mod readiness_test_support;
