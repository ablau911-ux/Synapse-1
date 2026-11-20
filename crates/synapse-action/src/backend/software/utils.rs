use std::{
    thread,
    time::{Duration, Instant},
};

use enigo::{Enigo, Settings};

use crate::ActionError;

pub(super) fn enigo() -> Result<Enigo, ActionError> {
    Enigo::new(&Settings::default()).map_err(|err| ActionError::BackendUnavailable {
        detail: format!("failed to initialize enigo: {err}"),
    })
}

pub(super) fn enigo_preserving_held_keys() -> Result<Enigo, ActionError> {
    Enigo::new(&Settings {
        release_keys_when_dropped: false,
        ..Settings::default()
    })
    .map_err(|err| ActionError::BackendUnavailable {
        detail: format!("failed to initialize enigo: {err}"),
    })
}

pub(super) fn enigo_error(context: &'static str) -> impl FnOnce(enigo::InputError) -> ActionError {
    move |err| ActionError::BackendUnavailable {
        detail: format!("{context}: {err}"),
    }
}

pub(super) fn sleep_ms(milliseconds: u32) -> bool {
    if milliseconds == 0 {
        return false;
    }
    let epoch = crate::hotkey::operator_release_epoch();
    let deadline = Instant::now() + Duration::from_millis(u64::from(milliseconds));
    sleep_until(deadline, epoch)
}

/// Timed software input (a drag with a duration, combo `at_ms` steps): each
/// step waits for an absolute offset from the start, so earlier delays never
/// accumulate. A release-all or the operator hotkey since the start ends the
/// wait with `SafetyOperatorHotkeyFired`.
pub(super) struct InputTimeline {
    started: Instant,
    release_epoch: u64,
}

impl InputTimeline {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            release_epoch: crate::hotkey::operator_release_epoch(),
        }
    }

    pub(super) fn wait_for_offset(&self, offset_ms: u64) -> Result<(), ActionError> {
        let deadline = self.started + Duration::from_millis(offset_ms);
        if sleep_until(deadline, self.release_epoch) {
            return Err(ActionError::SafetyOperatorHotkeyFired {
                detail: "operator release requested during timed software input".to_owned(),
            });
        }
        Ok(())
    }
}

fn sleep_until(deadline: Instant, epoch: u64) -> bool {
    loop {
        if crate::hotkey::operator_release_requested_since(epoch) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(1)));
    }
}
