//! Intended OS-input delivery target (OS-input safety invariant).
//!
//! Every OS-level input batch (`SendInput`, Enigo key, `SetPhysicalCursorPos`)
//! is delivered by Windows to whatever window is foreground at that instant.
//! An action therefore carries the window it was *meant* for — the top-level
//! root HWND plus owning pid — and the software backend re-verifies the live
//! foreground against it immediately before each batch. Keyboard/text input
//! with no intended target is refused rather than typed into whatever the human
//! happens to be using.

use synapse_core::{Point, Rect, types::StoredInputTarget};

use crate::{ActionError, ActionResult};

pub(crate) const STORED_AUTOMATION_TARGET_SOURCE: &str = "stored_automation_target";

/// Remediation carried by every `ACTION_INPUT_TARGET_REQUIRED` refusal.
pub const INPUT_TARGET_REMEDIATION: &str = "Pass window_hwnd, set a session target (target set), or focus an explicit window first (act focus_window); prefer a background route (CDP, UIA ValuePattern, WM_SETTEXT).";

/// Chooses the screen point where one wheel event lands.
///
/// The explicit point wins, else the current cursor; the chosen point must lie
/// inside `target_rect`. The cursor is never moved and a point outside the
/// target is never used.
///
/// # Errors
///
/// Returns `ACTION_INPUT_TARGET_REQUIRED` when the chosen point is unknown or
/// outside the target. An explicit point is authoritative: a cursor inside the
/// target does not rescue an explicit point outside it.
pub fn wheel_delivery_point(
    target_rect: Rect,
    cursor: Option<Point>,
    explicit_point: Option<Point>,
) -> ActionResult<Point> {
    let chosen = explicit_point.or(cursor);
    if let Some(point) = chosen
        && target_rect.contains(point)
    {
        return Ok(point);
    }
    let source = if explicit_point.is_some() {
        "the given screen point"
    } else {
        "the cursor (used when x/y are omitted)"
    };
    let at = chosen.map_or_else(|| "unknown".to_owned(), |p| format!("({}, {})", p.x, p.y));
    let Rect { x, y, w, h } = target_rect;
    let remedy = if w <= 0 || h <= 0 {
        "The target window has no on-screen area; restore or show it first.".to_owned()
    } else {
        format!(
            "Pass screen coordinates inside that rect, e.g. its centre x={}, y={}.",
            x.saturating_add(w / 2),
            y.saturating_add(h / 2)
        )
    };
    Err(ActionError::InputTargetRequired {
        detail: format!(
            "Wheel input refused before delivery, nothing was sent: {source} at {at} is outside the target window (screen rect x={x}, y={y}, w={w}, h={h}). {remedy}"
        ),
    })
}

/// Selects an automation's delivery window from an explicit binding and live proof.
///
/// # Errors
///
/// Returns `ACTION_INPUT_TARGET_REQUIRED` for an unbound automation or
/// `ACTION_INPUT_TARGET_NOT_READY` when its window is not claimed and in front.
pub fn require_stored_input_target(
    automation_id: &str,
    stored: Option<StoredInputTarget>,
    claimed_foreground: Option<StoredInputTarget>,
) -> ActionResult<StoredInputTarget> {
    let stored = stored.ok_or_else(|| ActionError::InputTargetRequired {
        detail: format!(
            "{automation_id}: no stored target window; set one when creating or arming"
        ),
    })?;
    if claimed_foreground != Some(stored) {
        return Err(ActionError::InputTargetNotReady {
            detail: format!(
                "{automation_id}: stored window {:#x} (pid {}) is not claimed and in front",
                stored.root_hwnd, stored.pid
            ),
        });
    }
    Ok(stored)
}

/// #315: maps a failed target-window check onto the action error space.
///
/// The check is the one `synapse_a11y` HWND rule. A closed window is
/// `TARGET_WINDOW_NOT_FOUND`; any other failure stays `ACTION_TARGET_INVALID`.
/// `context` names the caller.
#[must_use]
pub fn window_check_error(context: &str, error: synapse_a11y::A11yError) -> ActionError {
    match error {
        synapse_a11y::A11yError::WindowNotFound { detail } => ActionError::WindowNotFound {
            detail: format!("{context}: {detail}"),
        },
        other => ActionError::TargetInvalid {
            detail: format!("{context}: {other}"),
        },
    }
}

/// #315: checks that a target window is still alive.
///
/// # Errors
///
/// See [`window_check_error`].
pub fn ensure_live_window(hwnd: i64, context: &str) -> ActionResult<()> {
    synapse_a11y::ensure_live_window(hwnd).map_err(|error| window_check_error(context, error))
}

/// #315: the live top-level root of a target window (the window and its root
/// must both be alive).
///
/// # Errors
///
/// See [`window_check_error`].
pub fn live_root_hwnd(hwnd: i64, context: &str) -> ActionResult<i64> {
    synapse_a11y::top_level_root_hwnd(hwnd).map_err(|error| window_check_error(context, error))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDeliveryTarget {
    /// Top-level root HWND (`GetAncestor(.., GA_ROOT)`) in Synapse wire form.
    pub root_hwnd: i64,
    /// Process id owning `root_hwnd`.
    pub pid: u32,
    /// Which act path resolved the target (audit/diagnostics only).
    pub source: &'static str,
}

impl InputDeliveryTarget {
    #[must_use]
    pub const fn from_stored(target: StoredInputTarget) -> Self {
        Self {
            root_hwnd: target.root_hwnd,
            pid: target.pid,
            source: STORED_AUTOMATION_TARGET_SOURCE,
        }
    }

    #[must_use]
    pub const fn stored(&self) -> StoredInputTarget {
        StoredInputTarget {
            root_hwnd: self.root_hwnd,
            pid: self.pid,
        }
    }

    /// Resolves an HWND (any descendant is accepted) to its live root HWND and
    /// owning pid.
    ///
    /// # Errors
    ///
    /// Returns `TARGET_WINDOW_NOT_FOUND` when the window is closed,
    /// `ACTION_TARGET_INVALID` for a null or otherwise unusable HWND, or
    /// `ACTION_BACKEND_UNAVAILABLE` on non-Windows platforms.
    pub fn resolve_window(hwnd: i64, source: &'static str) -> ActionResult<Self> {
        resolve_window_impl(hwnd, source)
    }

    /// Checks that a live window belongs to this bound HWND and process.
    ///
    /// # Errors
    ///
    /// Returns an error if the window is stale or its live root identity differs.
    pub fn ensure_window_identity(&self, hwnd: i64) -> ActionResult<()> {
        let live = Self::resolve_window(hwnd, self.source)?;
        if live != *self {
            return Err(ActionError::TargetInvalid {
                detail: format!(
                    "input refused before delivery: live root {:#x} (pid {}) does not match intended root {:#x} (pid {})",
                    live.root_hwnd, live.pid, self.root_hwnd, self.pid
                ),
            });
        }
        Ok(())
    }

    /// `self` is a freshly resolved live identity. When `bound` (the handle's
    /// target) is the same root window, its pid must still match: a root HWND
    /// reused by another process is refused, never silently retargeted. A
    /// matching bound target is kept, so the guard keeps its proven pid.
    ///
    /// # Errors
    ///
    /// Returns `ACTION_TARGET_INVALID` when `bound` names this root HWND with a
    /// different pid.
    pub(crate) fn keep_bound_identity(self, bound: Option<Self>) -> ActionResult<Self> {
        match bound {
            Some(bound) if bound.root_hwnd == self.root_hwnd && bound.pid != self.pid => {
                Err(ActionError::TargetInvalid {
                    detail: format!(
                        "intended input target root HWND {:#x} now belongs to pid {} instead of the bound pid {} (source={}); the window was closed and its handle reused, so no input is sent",
                        self.root_hwnd, self.pid, bound.pid, bound.source
                    ),
                })
            }
            Some(bound) if bound.root_hwnd == self.root_hwnd => Ok(bound),
            _ => Ok(self),
        }
    }
}

#[cfg(test)]
#[path = "tests/delivery_identity_294.rs"]
mod delivery_identity_294;

#[cfg(test)]
#[path = "delivery_target_stored_table.rs"]
mod stored_table;

#[cfg(windows)]
fn resolve_window_impl(hwnd: i64, source: &'static str) -> ActionResult<InputDeliveryTarget> {
    let identity = synapse_a11y::window_root_identity(hwnd).map_err(|error| {
        window_check_error(
            &format!("intended input target HWND {hwnd:#x} (source={source}) is not usable"),
            error,
        )
    })?;
    Ok(InputDeliveryTarget {
        root_hwnd: identity.root_hwnd,
        pid: identity.pid,
        source,
    })
}

#[cfg(not(windows))]
fn resolve_window_impl(hwnd: i64, source: &'static str) -> ActionResult<InputDeliveryTarget> {
    Err(ActionError::BackendUnavailable {
        detail: format!(
            "intended input target HWND {hwnd:#x} (source={source}) requires Windows to resolve"
        ),
    })
}
