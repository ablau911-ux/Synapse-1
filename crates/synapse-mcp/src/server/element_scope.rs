//! #2406: the one rule "an element is acted on only inside the session's
//! target". `target_act` reads the facts (the session target, a native
//! element's top-level window, the pages of the target window, the frames
//! under the target page) and asks `ElementScope` before it acts on an element
//! named by its id. Pure: no OS call here.

use synapse_core::{ElementId, error_codes};

use crate::server::SessionTarget;

/// The session target, and the pages its window's raw-CDP endpoint lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScopeTarget {
    session: Option<SessionTarget>,
    /// The page ids the target window's raw-CDP endpoint lists (read only for
    /// a window target and a page element; empty when there is no endpoint).
    window_pages: Vec<String>,
}

impl ScopeTarget {
    pub(super) const fn new(session: Option<SessionTarget>, window_pages: Vec<String>) -> Self {
        Self {
            session,
            window_pages,
        }
    }

    /// The page this target shows: a page target's own page; for a window
    /// target, its page when the endpoint lists exactly one; else `None`.
    pub(super) fn page(&self) -> Option<&str> {
        match (&self.session, self.window_pages.as_slice()) {
            (Some(SessionTarget::Cdp { cdp_target_id, .. }), _) => Some(cdp_target_id.as_str()),
            (Some(SessionTarget::Window { .. }), [page]) => Some(page.as_str()),
            _ => None,
        }
    }
}

/// The element as the caller named it, and what was read about it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScopeElement {
    element_id: String,
    /// A native element's top-level window (`ElementRoot::root_hwnd`); `None`
    /// for a page element.
    root_hwnd: Option<i64>,
    /// The frame targets attached under the target page now
    /// (`cdp_list_frames`); empty when not read.
    page_frames: Vec<String>,
}

impl ScopeElement {
    pub(super) fn new(element_id: &str, root_hwnd: Option<i64>, page_frames: Vec<String>) -> Self {
        Self {
            element_id: element_id.trim().to_owned(),
            root_hwnd,
            page_frames,
        }
    }
}

/// The answer of `ElementScope::verdict`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ScopeVerdict {
    Allowed,
    /// Nothing may be sent; `code` is a `synapse_core::error_codes` code.
    Refused {
        code: &'static str,
        text: String,
    },
}

/// One element named by id, judged against the session target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ElementScope {
    target: ScopeTarget,
    element: ScopeElement,
}

impl ElementScope {
    pub(super) const fn new(target: ScopeTarget, element: ScopeElement) -> Self {
        Self { target, element }
    }

    /// Allowed only when the element is inside the session target: a native
    /// element in the target window; a page element of the target window on
    /// the target's own page or a frame attached under it. `verb` starts every
    /// refusal text.
    pub(super) fn verdict(&self, verb: &str) -> ScopeVerdict {
        let Ok(element_id) = ElementId::parse(&self.element.element_id) else {
            return ScopeVerdict::Refused {
                code: error_codes::TOOL_PARAMS_INVALID,
                text: format!(
                    "target_act verb={verb} element_id {:?} cannot be read as an element id from find; nothing was sent",
                    self.element.element_id
                ),
            };
        };
        let Some(session) = &self.target.session else {
            return ScopeVerdict::Refused {
                code: error_codes::TARGET_NOT_SET,
                text: format!(
                    "target_act verb={verb} by element_id requires this MCP session to own a target; set one with set_target (target operation=set) and claim it (target operation=claim) first; nothing was sent"
                ),
            };
        };
        let target_window = match session {
            SessionTarget::Window { hwnd } => *hwnd,
            SessionTarget::Cdp { window_hwnd, .. } => *window_hwnd,
        };
        let page_element = synapse_a11y::cdp_backend_from_element_id(&element_id).is_some();
        let element_window = if page_element {
            element_id.parts().ok().map(|parts| parts.hwnd)
        } else {
            self.element.root_hwnd
        };
        if element_window != Some(target_window) {
            return ScopeVerdict::Refused {
                code: error_codes::ACTION_TARGET_INVALID,
                text: format!(
                    "target_act verb={verb} refused: element {element_id} belongs to another window, not the session target window HWND 0x{target_window:x}; nothing was sent"
                ),
            };
        }
        if !page_element {
            return ScopeVerdict::Allowed;
        }
        let Some(target_page) = self.target.page() else {
            return ScopeVerdict::Refused {
                code: error_codes::ACTION_TARGET_INVALID,
                text: format!(
                    "target_act verb={verb} refused: the page of the session target window HWND 0x{target_window:x} cannot be told (its raw CDP endpoint lists {} pages, not exactly one); target the page itself with cdp_open_tab first; nothing was sent",
                    self.target.window_pages.len()
                ),
            };
        };
        let on_target_page =
            synapse_a11y::cdp_target_from_element_id(&element_id).is_some_and(|page| {
                page.eq_ignore_ascii_case(target_page)
                    || self
                        .element
                        .page_frames
                        .iter()
                        .any(|frame| frame.eq_ignore_ascii_case(&page))
            });
        if on_target_page {
            return ScopeVerdict::Allowed;
        }
        ScopeVerdict::Refused {
            code: error_codes::ACTION_TARGET_INVALID,
            text: format!(
                "target_act verb={verb} refused: element {element_id} belongs to another page, not the session target page {target_page} or a frame under it; nothing was sent"
            ),
        }
    }
}

#[cfg(test)]
#[path = "element_scope_table_2406.rs"]
mod element_scope_table_2406;
