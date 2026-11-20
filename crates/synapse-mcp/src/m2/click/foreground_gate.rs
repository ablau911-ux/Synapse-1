//! Gate for the act_click real-foreground (SendInput) tier (#187).
//!
//! SendInput moves the real cursor and clicks whatever window is under it, so
//! the human's desktop is only safe if the tier is reached through a sanctioned
//! route. Other foreground tiers already require `break_glass` /
//! `full_capability`, or a claimed route (this session owns the claim on the
//! target, the target is the session target, and it is the OS foreground).
//! This module holds the pure decisions; the server layer gathers the claim
//! facts and `acquire_click_foreground_lease` enforces the result right before
//! input is sent.

use rmcp::ErrorData;
use synapse_core::{Point, Rect, error_codes};

use crate::m1::mcp_error;

/// How a SendInput click was authorized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForegroundClickAuth {
    /// `break_glass` / `full_capability` (or the unscoped stdio admin): the raw
    /// human-foreground tier is sanctioned by profile.
    Profile,
    /// This session claimed `root_hwnd`, it is the session target; the live OS
    /// foreground and click point are checked again just before SendInput.
    Claimed { root_hwnd: i64 },
}

/// Claim facts about the click target, read by the server layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClaimFacts {
    /// Top-level window the click targets (element root or
    /// `verify_target_window_hwnd`); `None` for a bare screen coordinate.
    pub(crate) target_root: Option<i64>,
    /// This session holds a live `target_claim` on that window.
    pub(crate) claim_held: bool,
    /// That window is this session's current target.
    pub(crate) session_target_matches: bool,
}

pub(crate) fn authorize_foreground_click(
    profile_allows_foreground_tier: bool,
    facts: &ClaimFacts,
) -> Result<ForegroundClickAuth, ErrorData> {
    if profile_allows_foreground_tier {
        return Ok(ForegroundClickAuth::Profile);
    }
    let Some(root_hwnd) = facts.target_root else {
        return Err(denied(
            "act_click real-foreground input has no claimed target window: pass an element or verify_target_window_hwnd for a window this session claimed (target_claim), or escalate with act operation=foreground (break_glass)",
        ));
    };
    if !facts.claim_held {
        return Err(denied(&format!(
            "act_click real-foreground input refused: window 0x{root_hwnd:x} is not claimed by this session; call target_claim first or escalate with act operation=foreground (break_glass)"
        )));
    }
    if !facts.session_target_matches {
        return Err(denied(&format!(
            "act_click real-foreground input refused: window 0x{root_hwnd:x} is claimed but is not this session's target; select it as the session target first"
        )));
    }
    Ok(ForegroundClickAuth::Claimed { root_hwnd })
}

/// Live check right before SendInput on the claimed route: the claimed window
/// must be the OS foreground and the click point must fall inside it, otherwise
/// the click would land in whatever window covers the target.
pub(crate) fn verify_claimed_click_foreground(
    root_hwnd: i64,
    foreground_root: Option<i64>,
    foreground_bounds: Option<Rect>,
    point: Option<Point>,
) -> Result<(), ErrorData> {
    if foreground_root != Some(root_hwnd) {
        let actual =
            foreground_root.map_or_else(|| "none".to_owned(), |hwnd| format!("0x{hwnd:x}"));
        return Err(mcp_error(
            error_codes::FOREGROUND_ACTIVATION_REFUSED,
            format!(
                "act_click refused before SendInput: claimed target window 0x{root_hwnd:x} is not the OS foreground (foreground root: {actual}); no input was sent"
            ),
        ));
    }
    let Some(point) = point else {
        return Ok(());
    };
    if foreground_bounds.is_some_and(|bounds| bounds.contains(point)) {
        return Ok(());
    }
    Err(mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        format!(
            "act_click refused before SendInput: point ({}, {}) is not provably inside claimed target window 0x{root_hwnd:x}; no input was sent",
            point.x, point.y
        ),
    ))
}

/// Read the live OS foreground and apply [`verify_claimed_click_foreground`].
/// `Profile` authorization needs no live proof.
pub(crate) fn enforce_foreground_click_auth(
    auth: ForegroundClickAuth,
    point: Option<Point>,
) -> Result<(), ErrorData> {
    let ForegroundClickAuth::Claimed { root_hwnd } = auth else {
        return Ok(());
    };
    let (foreground_root, bounds) = match synapse_a11y::current_foreground_context() {
        Ok(foreground) => {
            let root = synapse_a11y::top_level_root_hwnd(foreground.hwnd).map_err(|error| {
                let error = synapse_a11y::as_foreground_error(error);
                mcp_error(
                    error.code(),
                    format!(
                        "act_click foreground HWND 0x{:x} root readback failed: {error}",
                        foreground.hwnd
                    ),
                )
            })?;
            (Some(root), Some(foreground.window_bounds))
        }
        Err(error) if error.code() == error_codes::A11Y_NO_FOREGROUND => (None, None),
        Err(error) => {
            return Err(mcp_error(
                error.code(),
                format!("act_click foreground readback failed: {error}"),
            ));
        }
    };
    verify_claimed_click_foreground(root_hwnd, foreground_root, bounds, point)
}

fn denied(detail: &str) -> ErrorData {
    mcp_error(error_codes::TOOL_PROFILE_POLICY_DENIED, detail.to_owned())
}
