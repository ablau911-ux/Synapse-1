//! #2350 / #2359: `act verb=drag`. `DragRoute` picks the existing drag
//! primitive for the session target; `DragPath` checks the two ends and the
//! duration and gives what the primitive sends (two points, or two selectors
//! on a Chrome bridge tab). Pure: the target kind, whether this session holds
//! its target's claim and whether that window is in front, the element facts
//! (`ElementScope`, #2406) and the bounds are read by the caller and passed
//! in. A refusal is an `ErrorData`: nothing was sent.

use rmcp::ErrorData;
use synapse_core::{Point, Rect, error_codes};

use super::act_route::ActTarget;
use super::element_scope::{ElementScope, ScopeVerdict};
use crate::m1::mcp_error;

/// The drag duration when the caller gives none (the bridge drag's default).
const DEFAULT_DURATION_MS: u32 = 350;
/// The longest drag (the bridge drag's limit).
const MAX_DURATION_MS: u32 = 10_000;

/// The existing primitive that sends a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DragBackend {
    /// `browser_drag` in the session's own Chrome bridge tab.
    BridgeDrag,
    /// The raw-CDP stroke of `act_stroke` into the session's page.
    RawCdpStroke,
    /// The real-input stroke of `act_stroke` (input lease, a claim check on
    /// every window under the path, abort when focus is lost).
    RealInputStroke,
}

/// Which primitive drags on the session target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DragRoute {
    target: ActTarget,
    /// This session holds the claim of its target.
    claimed: bool,
    /// The target's window is the OS foreground window.
    in_front: bool,
}

impl DragRoute {
    pub(super) const fn new(target: ActTarget, claimed: bool, in_front: bool) -> Self {
        Self {
            target,
            claimed,
            in_front,
        }
    }

    /// A Chrome bridge tab: the bridge drag; a raw-CDP page: the raw-CDP
    /// stroke; a native window: the real-input stroke, only when this session
    /// claimed it and it is in front (no background drag exists).
    pub(super) fn backend(&self) -> Result<DragBackend, ErrorData> {
        match self.target {
            ActTarget::None => Err(mcp_error(
                error_codes::TARGET_NOT_SET,
                "target_act verb=drag requires this MCP session to own a target; set one with set_target (target operation=set) and claim it (target operation=claim) first; nothing was sent",
            )),
            ActTarget::BridgeTab => Ok(DragBackend::BridgeDrag),
            ActTarget::RawCdpPage => Ok(DragBackend::RawCdpStroke),
            // The drag caller never asks the #79 WebView2 question; a WebView2
            // window behind would get a native window's rule.
            ActTarget::NativeWindow
            | ActTarget::NativeWindowWithRawCdp
            | ActTarget::WebViewBehindWithoutCdp
                if !self.claimed =>
            {
                Err(mcp_error(
                    error_codes::TARGET_CLAIM_NOT_FOUND,
                    "target_act verb=drag refused: a drag on a native window sends real mouse input, so this session must hold the window's claim (target operation=claim) first; nothing was sent",
                ))
            }
            ActTarget::NativeWindow
            | ActTarget::NativeWindowWithRawCdp
            | ActTarget::WebViewBehindWithoutCdp
                if !self.in_front =>
            {
                Err(mcp_error(
                    error_codes::TOOL_PROFILE_POLICY_DENIED,
                    "target_act verb=drag refused: a drag on a native window needs the session's claimed window in front (real mouse input; no background drag exists); bring it in front first; nothing was sent",
                ))
            }
            ActTarget::NativeWindow
            | ActTarget::NativeWindowWithRawCdp
            | ActTarget::WebViewBehindWithoutCdp => Ok(DragBackend::RealInputStroke),
        }
    }
}

/// Where the ends of a drag must lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DragArea {
    /// A Chrome bridge tab: both ends are CSS selectors the page resolves.
    BridgeTab,
    /// A window (screen pixels) or a raw-CDP page (viewport CSS pixels, from
    /// 0,0): ends are points or elements inside these bounds.
    Bounds(Rect),
}

/// One end of a drag as the caller named it, with what was read about it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DragEnd {
    /// x/y in the target's own coordinates: from the top-left corner of the
    /// area's bounds.
    Point(Point),
    /// An element id from find: its scope (#2406) and its box in the area's
    /// pixels (a box of size 0 when only its centre was read).
    Element { scope: ElementScope, bounds: Rect },
    /// A CSS selector (Chrome bridge tab only).
    Selector(String),
}

/// The two ends to send, in the primitive's own terms.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DragEnds {
    /// Window: screen pixels; raw-CDP page: viewport CSS pixels.
    Points(Point, Point),
    Selectors(String, String),
}

/// A checked drag: two different ends inside the target, and a duration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DragPath {
    ends: DragEnds,
    duration_ms: u32,
}

impl DragPath {
    /// Refuses a missing end, an end of the wrong kind for the area, an
    /// element outside the session target (`ElementScope`), an end outside the
    /// bounds, equal ends, and a duration outside 1..=10000 (default 350).
    pub(super) fn new(
        from: Option<DragEnd>,
        to: Option<DragEnd>,
        duration_ms: Option<i64>,
        area: DragArea,
    ) -> Result<Self, ErrorData> {
        let duration_ms = match duration_ms {
            None => DEFAULT_DURATION_MS,
            Some(requested) => match u32::try_from(requested) {
                Ok(ms) if (1..=MAX_DURATION_MS).contains(&ms) => ms,
                _ => {
                    return Err(mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        format!(
                            "target_act verb=drag duration_ms must be in 1..={MAX_DURATION_MS} (default {DEFAULT_DURATION_MS}), got {requested}; nothing was sent"
                        ),
                    ));
                }
            },
        };
        let (Some(from), Some(to)) = (from, to) else {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "target_act verb=drag needs both ends: the start (element_id, selector, or x and y) and the end (to_element_id, to_selector, or to_x and to_y); nothing was sent",
            ));
        };
        let ends = match area {
            DragArea::BridgeTab => match (from, to) {
                (DragEnd::Selector(from), DragEnd::Selector(to)) => {
                    DragEnds::Selectors(from.trim().to_owned(), to.trim().to_owned())
                }
                _ => {
                    return Err(mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        "target_act verb=drag on a Chrome bridge tab names both ends by CSS selector (selector and to_selector); nothing was sent",
                    ));
                }
            },
            DragArea::Bounds(bounds) => {
                DragEnds::Points(Self::point(from, bounds)?, Self::point(to, bounds)?)
            }
        };
        let same_place = match &ends {
            DragEnds::Points(from, to) => from == to,
            DragEnds::Selectors(from, to) => from == to,
        };
        if same_place {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "target_act verb=drag start and end are the same place: nothing to drag; nothing was sent",
            ));
        }
        Ok(Self { ends, duration_ms })
    }

    /// Where one end lies, in the area's pixels; refused outside the target.
    fn point(end: DragEnd, bounds: Rect) -> Result<Point, ErrorData> {
        let point = match end {
            DragEnd::Selector(_) => {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "target_act verb=drag takes a CSS selector end only on a Chrome bridge tab; give element_id / to_element_id or x, y / to_x, to_y; nothing was sent",
                ));
            }
            DragEnd::Point(point) => Point {
                x: bounds.x.saturating_add(point.x),
                y: bounds.y.saturating_add(point.y),
            },
            DragEnd::Element {
                scope,
                bounds: element,
            } => {
                if let ScopeVerdict::Refused { code, text } = scope.verdict("drag") {
                    return Err(mcp_error(code, text));
                }
                Point {
                    x: element.x.saturating_add(element.w / 2),
                    y: element.y.saturating_add(element.h / 2),
                }
            }
        };
        if !bounds.contains(point) {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb=drag end ({}, {}) is outside the target ({}, {}, {}x{}); nothing was sent",
                    point.x, point.y, bounds.x, bounds.y, bounds.w, bounds.h
                ),
            ));
        }
        Ok(point)
    }

    /// The two points to stroke through; `None` on a Chrome bridge tab.
    pub(super) fn points(&self) -> Option<(Point, Point)> {
        match &self.ends {
            DragEnds::Points(from, to) => Some((*from, *to)),
            DragEnds::Selectors(..) => None,
        }
    }

    /// The two selectors of a Chrome bridge tab drag; `None` elsewhere.
    pub(super) fn selectors(&self) -> Option<(&str, &str)> {
        match &self.ends {
            DragEnds::Selectors(from, to) => Some((from.as_str(), to.as_str())),
            DragEnds::Points(..) => None,
        }
    }

    /// The duration in milliseconds.
    pub(super) const fn duration_ms(&self) -> u32 {
        self.duration_ms
    }
}

#[cfg(test)]
#[path = "drag_route_table_2350.rs"]
mod drag_route_table_2350;
