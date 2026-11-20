use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex, MutexGuard},
};

use synapse_core::{
    Action, AimCurve, AimStyle, AimTarget, GamepadReport, Key, KeyCode, MouseButton, MouseTarget,
    PadButton, PadId, Point, Stick, Trigger,
};

use crate::{ActionBackend, ActionError, EmitState};

mod log;
mod state;

pub use log::MAX_RECORDED_EVENTS;
use state::RecordingState;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordedInput {
    KeyDown {
        key: Key,
    },
    KeyUp {
        key: Key,
    },
    DelayMs {
        ms: u32,
    },
    UnicodeUnitDown {
        unit: u16,
    },
    UnicodeUnitUp {
        unit: u16,
    },
    MouseMove {
        to: MouseTarget,
        curve: AimCurve,
        duration_ms: u32,
    },
    MouseMoveAbsolute {
        point: Point,
    },
    MouseMoveRelative {
        dx: f64,
        dy: f64,
    },
    MouseButtonDown {
        button: MouseButton,
    },
    MouseButtonUp {
        button: MouseButton,
    },
    MouseStrokePoint {
        elapsed_ms: f64,
        point: Point,
    },
    MouseScroll {
        dy: i32,
        dx: i32,
        at: Option<Point>,
    },
    AimAt {
        target: AimTarget,
        style: AimStyle,
        deadline_ms: u32,
    },
    ComboAt {
        at_ms: u32,
    },
    PadButtonDown {
        pad: PadId,
        button: PadButton,
    },
    PadButtonUp {
        pad: PadId,
        button: PadButton,
    },
    PadStick {
        pad: PadId,
        stick: Stick,
        x: f32,
        y: f32,
    },
    PadTrigger {
        pad: PadId,
        trigger: Trigger,
        value: f32,
    },
    PadReport {
        pad: PadId,
        report: GamepadReport,
    },
    ReleaseAll {
        held_keys: Vec<KeyCode>,
        held_buttons: Vec<MouseButton>,
        pads: Vec<PadId>,
    },
}

impl RecordedInput {
    /// Label that is safe to log: identical to the `Debug` form except that
    /// typed-text carriers (Unicode units and key identities, see
    /// [`redacted_key_label`]) never appear, so a password typed through the
    /// recording backend cannot reach the log (#196).
    #[must_use]
    pub fn log_label(&self) -> String {
        match self {
            Self::UnicodeUnitDown { .. } => "unicode_unit_down".to_owned(),
            Self::UnicodeUnitUp { .. } => "unicode_unit_up".to_owned(),
            Self::KeyDown { key } => format!("key_down:{}", redacted_key_label(key)),
            Self::KeyUp { key } => format!("key_up:{}", redacted_key_label(key)),
            other => format!("{other:?}"),
        }
    }
}

/// Log-safe key label: control-style named keys keep their name; printable
/// characters (a `Symbol` key, a one-character name) and raw HID codes are
/// redacted because together they reconstruct typed text.
#[must_use]
pub fn redacted_key_label(key: &Key) -> String {
    match &key.code {
        KeyCode::Named { value } if value.chars().count() > 1 => value.clone(),
        KeyCode::Named { .. } | KeyCode::Symbol { .. } => "char".to_owned(),
        KeyCode::HidCode { .. } => "hid".to_owned(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct RecordingBackend {
    inner: Arc<Mutex<RecordingState>>,
}

impl RecordingBackend {
    #[must_use]
    #[tracing::instrument(fields(backend = "recording"))]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clone of the retained event window (at most [`MAX_RECORDED_EVENTS`]).
    /// O(n): for tests and diagnostics, not per-action readbacks - use
    /// [`Self::execute_capturing`] or [`Self::events_since`] for those.
    #[must_use]
    pub fn events(&self) -> Vec<RecordedInput> {
        self.state().events.retained()
    }

    /// Events recorded since creation, including ones evicted from the window.
    #[must_use]
    pub fn event_count(&self) -> usize {
        self.state().events.total()
    }

    /// Retained events recorded after the first `event_count` events.
    #[must_use]
    pub fn events_since(&self, event_count: usize) -> Vec<RecordedInput> {
        self.state().events.since(event_count)
    }

    /// Execute `action` and return exactly the events it recorded. The state
    /// lock is held across the call, so concurrent sessions sharing this
    /// backend never see each other's events in their readback (#196).
    ///
    /// # Errors
    /// Returns the validation or emission error from the action.
    pub fn execute_capturing(
        &self,
        action: &Action,
        state: &mut EmitState,
    ) -> Result<Vec<RecordedInput>, ActionError> {
        crate::validate_action(action)?;
        let mut recording = self.state();
        let total_before = recording.events.total();
        recording.apply_action(action, state)?;
        Ok(recording.events.since(total_before))
    }

    #[must_use]
    pub fn held_keys(&self) -> BTreeSet<KeyCode> {
        self.state().held_keys.clone()
    }

    #[must_use]
    pub fn held_buttons(&self) -> BTreeSet<MouseButton> {
        self.state().held_buttons.clone()
    }

    #[must_use]
    pub fn pad_state(&self) -> HashMap<PadId, GamepadReport> {
        self.state().pad_state.clone()
    }

    fn state(&self) -> MutexGuard<'_, RecordingState> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl ActionBackend for RecordingBackend {
    #[tracing::instrument(skip_all, fields(backend = "recording"))]
    fn execute(&self, action: &Action, state: &mut EmitState) -> Result<(), ActionError> {
        crate::validate_action(action)?;
        self.state().apply_action(action, state)
    }
}

#[cfg(test)]
#[path = "recording/tests/event_log_196.rs"]
mod event_log_196_tests;
