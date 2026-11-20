//! 300f / #107 part d: the top-level window a UI Automation element belongs
//! to, read at the moment of the action. `target_act` (element id and
//! name + role routes) and the point click route (`m2::click::point_post`)
//! both refuse, before any UIA call, an element whose root is not the
//! claimed window.

use rmcp::ErrorData;
use synapse_core::{ElementId, error_codes};

use crate::m1::mcp_error;

/// One element's own HWND and its top-level root, as read now.
pub(crate) struct ElementRoot {
    element: String,
    action: String,
    element_hwnd: i64,
    root_hwnd: i64,
}

impl ElementRoot {
    /// Reads the element's HWND and its top-level root. `action` starts every
    /// message (`target_act verb=click`, `act_click point click`).
    pub(crate) fn read(element_id: &ElementId, action: &str) -> Result<Self, ErrorData> {
        let element_hwnd = element_id
            .parts()
            .map_err(|error| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("{action} element_id is invalid: {error}"),
                )
            })?
            .hwnd;
        let root_hwnd = synapse_a11y::top_level_root_hwnd(element_hwnd).map_err(|error| {
            mcp_error(
                error.code(),
                format!("{action} element HWND 0x{element_hwnd:x} root readback failed: {error}"),
            )
        })?;
        Ok(Self {
            element: element_id.to_string(),
            action: action.to_owned(),
            element_hwnd,
            root_hwnd,
        })
    }

    pub(crate) const fn element_hwnd(&self) -> i64 {
        self.element_hwnd
    }

    pub(crate) const fn root_hwnd(&self) -> i64 {
        self.root_hwnd
    }

    /// Refuses (`ACTION_TARGET_INVALID`) an element that belongs to another
    /// top-level window than the claimed `window_hwnd`.
    pub(crate) fn ensure_in(&self, window_hwnd: i64) -> Result<(), ErrorData> {
        if self.root_hwnd == window_hwnd {
            return Ok(());
        }
        Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "{} refused: element {} belongs to window HWND 0x{:x}, not the session target window HWND 0x{window_hwnd:x}",
                self.action, self.element, self.root_hwnd
            ),
        ))
    }
}
