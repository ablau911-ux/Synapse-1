//! Restore of the human's foreground after the lane's own focus step (#217).
//!
//! `act operation=foreground` captures the human's foreground window, then
//! focuses the claimed target itself. At unwind the current foreground is that
//! target, which is not a human move: the lane recorded it as its own focus and
//! the prior window must be put back. A foreground that is neither the prior
//! window nor the lane's own focus is still treated as a human move.
//!
//! Own focus is the focused window itself or a window it owns (its `GW_OWNER`
//! chain reaches the focused window: a modal or dialog the target opened).
//! Sharing a process id is not enough: every Chrome, Explorer or VS Code
//! window shares one pid, so a human switch between two windows of the same
//! process must stay a human move.

use serde_json::json;
use synapse_core::error_codes;

use super::{
    ForegroundInputContextSnapshot, ForegroundInputLeaseGuard, ForegroundRestoreCurrentDecision,
    foreground_restore_current_decision, record_foreground_restore_context_event,
};

/// Independently read OS foreground used by the restore decision.
pub(super) struct CurrentForeground {
    pub(super) hwnd: i64,
    pub(super) pid: u32,
    pub(super) title: String,
}

/// The foreground the lane's own focus step produced (#217). `pid` is kept for
/// readback evidence only; it does not decide ownership.
#[derive(Clone, Copy, Debug)]
pub(super) struct OwnFocus {
    pub(super) hwnd: i64,
    pub(super) pid: u32,
}

impl OwnFocus {
    /// The focused window itself, or a window whose owner chain (nearest owner
    /// first, as returned by `owner_chain(current_hwnd)`) reaches it. The owner
    /// chain is only read when the current window is not the focused one.
    fn covers(self, current_hwnd: i64, owner_chain: impl FnOnce(i64) -> Vec<i64>) -> bool {
        self.hwnd == current_hwnd || owner_chain(current_hwnd).contains(&self.hwnd)
    }
}

/// Decide the restore outcome. `owner_chain` returns the `GW_OWNER` chain of a
/// window (excluding the window itself); it is consulted only when the plain
/// decision is a human move and the lane recorded its own focus.
pub(super) fn foreground_restore_decision(
    captured_hwnd: i64,
    expected_pid: u32,
    current_hwnd: i64,
    current_pid: u32,
    own_focus: Option<OwnFocus>,
    owner_chain: impl FnOnce(i64) -> Vec<i64>,
) -> ForegroundRestoreCurrentDecision {
    let decision =
        foreground_restore_current_decision(captured_hwnd, expected_pid, current_hwnd, current_pid);
    let Some(own) = own_focus else {
        return decision;
    };
    if matches!(decision, ForegroundRestoreCurrentDecision::SkipHumanMoved)
        && own.covers(current_hwnd, owner_chain)
    {
        return ForegroundRestoreCurrentDecision::RestoreOwnFocus;
    }
    decision
}

/// Live owner-chain source for [`foreground_restore_decision`].
///
/// Reads the live `GW_OWNER` chain; a failed walk yields an empty chain, so the
/// window is treated as a human move (fail closed).
pub(super) fn live_owner_chain(hwnd: i64) -> Vec<i64> {
    synapse_a11y::window_owner_chain(hwnd)
        .into_iter()
        .map(|(owner, _)| owner)
        .collect()
}

/// Put back the prior foreground window that a background action's own target
/// took as a side effect (#300: a WinForms UIA provider activates its window
/// while serving Toggle or Invoke). The target's activation is treated like the
/// lane's own focus step, so the same restore and separate readback run.
/// Returns `"restored"` or `"failed"`.
pub(crate) fn restore_foreground_taken_by_target(
    tool: &'static str,
    session_id: &str,
    prior: synapse_a11y::WindowRootIdentity,
    target: synapse_a11y::WindowRootIdentity,
) -> &'static str {
    let snapshot = ForegroundInputContextSnapshot {
        cursor: None,
        foreground_hwnd: Some(prior.root_hwnd),
        foreground_pid: Some(prior.pid),
        foreground_process_started_at_100ns: None,
        foreground_title: None,
        cursor_capture_error: None,
        foreground_capture_error: None,
        own_focus: Some(OwnFocus {
            hwnd: target.root_hwnd,
            pid: target.pid,
        }),
    };
    snapshot.restore_own_focus(tool, session_id, prior.root_hwnd, prior.pid)
}

impl ForegroundInputLeaseGuard {
    /// Record the current OS foreground as the lane's own focus change. Call
    /// right after the verified claimed-target focus step.
    pub(crate) fn record_own_foreground_focus(&mut self) {
        if let Some(snapshot) = self.context_restore.as_mut() {
            snapshot.record_own_focus(self.tool, &self.session_id);
        }
    }
}

impl ForegroundInputContextSnapshot {
    /// Decide using the recorded own focus; restore only a lane-owned change.
    pub(super) fn restore_foreground_for_current(
        &self,
        tool: &'static str,
        session_id: &str,
        current: &CurrentForeground,
        owner_chain: impl FnOnce(i64) -> Vec<i64>,
        restore_own: impl FnOnce() -> &'static str,
    ) -> &'static str {
        let (Some(hwnd), Some(expected_pid)) = (self.foreground_hwnd, self.foreground_pid) else {
            return "skipped";
        };
        let (outcome, code, reason) = match foreground_restore_decision(
            hwnd,
            expected_pid,
            current.hwnd,
            current.pid,
            self.own_focus,
            owner_chain,
        ) {
            ForegroundRestoreCurrentDecision::RestoreOwnFocus => return restore_own(),
            ForegroundRestoreCurrentDecision::AlreadyCurrent => (
                "already_current",
                "INPUT_LEASE_CONTEXT_FOREGROUND_ALREADY_CURRENT",
                "foreground_already_current",
            ),
            ForegroundRestoreCurrentDecision::SkipHumanMoved => (
                "skipped_human_moved",
                error_codes::FOREGROUND_RESTORE_SKIPPED_HUMAN_MOVED,
                "foreground_restore_skipped_human_moved",
            ),
        };
        tracing::info!(
            code, tool, session_id, prior_hwnd = hwnd, expected_pid,
            current_hwnd = current.hwnd, current_pid = current.pid,
            current_title = %current.title, outcome,
            "readback=foreground_window restore decision"
        );
        record_foreground_restore_context_event(
            tool,
            session_id,
            outcome,
            code,
            reason,
            Some(
                json!({ "hwnd": current.hwnd, "pid": current.pid, "window_title": current.title }),
            ),
            None,
            json!({
                "prior_hwnd": hwnd, "expected_pid": expected_pid,
                "expected_process_started_at_100ns": self.foreground_process_started_at_100ns,
                "prior_title": self.foreground_title,
            }),
        );
        outcome
    }

    fn record_own_focus(&mut self, tool: &'static str, session_id: &str) {
        self.record_own_focus_with(tool, session_id, || {
            synapse_a11y::current_foreground_context().map(|current| (current.hwnd, current.pid))
        });
    }

    /// Record the lane's own focus from an injected foreground reader that
    /// returns the current OS foreground as `(hwnd, pid)`. A failed read leaves
    /// `own_focus` unchanged, so restore treats a foreground change as a human
    /// move.
    pub(super) fn record_own_focus_with<E: std::fmt::Display>(
        &mut self,
        tool: &'static str,
        session_id: &str,
        read_foreground: impl FnOnce() -> Result<(i64, u32), E>,
    ) {
        match read_foreground() {
            Ok((hwnd, pid)) => {
                tracing::info!(
                    code = "INPUT_LEASE_CONTEXT_OWN_FOCUS_RECORDED",
                    tool,
                    session_id,
                    own_focus_hwnd = hwnd,
                    own_focus_pid = pid,
                    "readback=foreground_window outcome=own_focus_recorded"
                );
                self.own_focus = Some(OwnFocus { hwnd, pid });
            }
            Err(error) => {
                tracing::warn!(
                    code = error_codes::ACTION_FOREGROUND_CONTEXT_RESTORE_SKIPPED,
                    tool,
                    session_id,
                    reason_code = "own_focus_readback_failed",
                    detail = %error,
                    "own foreground focus could not be read back; restore will treat a foreground change as a human move"
                );
            }
        }
    }

    /// Put the captured prior window back after the lane's own focus step and
    /// verify it with a separate foreground readback.
    pub(super) fn restore_own_focus(
        &self,
        tool: &'static str,
        session_id: &str,
        prior_hwnd: i64,
        expected_pid: u32,
    ) -> &'static str {
        let detail = json!({
            "prior_hwnd": prior_hwnd,
            "expected_pid": expected_pid,
            "own_focus_hwnd": self.own_focus.map(|own| own.hwnd),
            "own_focus_pid": self.own_focus.map(|own| own.pid),
            "prior_title": self.foreground_title,
        });
        if let Err(error) = synapse_a11y::focus_window_with_intent(
            prior_hwnd,
            synapse_a11y::ForegroundActivationIntent::LeaseContextRestore { caller: tool },
        ) {
            tracing::error!(
                code = error_codes::ACTION_FOREGROUND_CONTEXT_RESTORE_FAILED,
                tool,
                session_id,
                reason_code = "foreground_restore_failed",
                foreground_hwnd = prior_hwnd,
                foreground_pid = expected_pid,
                detail = %error,
                "foreground window restore after own focus failed"
            );
            record_foreground_restore_context_event(
                tool,
                session_id,
                "failed",
                error_codes::ACTION_FOREGROUND_CONTEXT_RESTORE_FAILED,
                "foreground_restore_failed",
                None,
                Some(json!({ "detail": error.to_string() })),
                detail,
            );
            return "failed";
        }
        match synapse_a11y::current_foreground_context() {
            Ok(after) if after.hwnd == prior_hwnd && after.pid == expected_pid => {
                tracing::info!(
                    code = "INPUT_LEASE_CONTEXT_FOREGROUND_RESTORED",
                    tool,
                    session_id,
                    foreground_hwnd = prior_hwnd,
                    foreground_pid = expected_pid,
                    after_title = %after.window_title,
                    "readback=foreground_window outcome=restored"
                );
                record_foreground_restore_context_event(
                    tool,
                    session_id,
                    "restored",
                    "INPUT_LEASE_CONTEXT_FOREGROUND_RESTORED",
                    "foreground_restored_after_own_focus",
                    Some(json!({
                        "hwnd": after.hwnd,
                        "pid": after.pid,
                        "process_name": after.process_name,
                        "window_title": after.window_title,
                    })),
                    None,
                    detail,
                );
                "restored"
            }
            Ok(after) => {
                tracing::error!(
                    code = error_codes::ACTION_FOREGROUND_CONTEXT_RESTORE_FAILED,
                    tool,
                    session_id,
                    reason_code = "foreground_readback_mismatch",
                    requested_hwnd = prior_hwnd,
                    expected_pid,
                    actual_hwnd = after.hwnd,
                    actual_pid = after.pid,
                    "foreground window restore readback mismatch after own focus"
                );
                record_foreground_restore_context_event(
                    tool,
                    session_id,
                    "failed",
                    error_codes::ACTION_FOREGROUND_CONTEXT_RESTORE_FAILED,
                    "foreground_readback_mismatch",
                    Some(json!({ "hwnd": after.hwnd, "pid": after.pid })),
                    None,
                    detail,
                );
                "failed"
            }
            Err(error) => {
                tracing::error!(
                    code = error_codes::ACTION_FOREGROUND_CONTEXT_RESTORE_FAILED,
                    tool,
                    session_id,
                    reason_code = "foreground_readback_failed",
                    detail = %error,
                    "foreground window restore readback failed after own focus"
                );
                record_foreground_restore_context_event(
                    tool,
                    session_id,
                    "failed",
                    error_codes::ACTION_FOREGROUND_CONTEXT_RESTORE_FAILED,
                    "foreground_readback_failed",
                    None,
                    Some(json!({ "detail": error.to_string() })),
                    detail,
                );
                "failed"
            }
        }
    }
}
