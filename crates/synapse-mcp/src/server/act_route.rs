//! #2348: which backend serves one DOM-style `act` verb on the session
//! target. Its one rule: the Chrome bridge is chosen only for a Chrome bridge
//! tab. A native window is served by native accessibility, a raw-CDP page by
//! raw CDP; what the target cannot serve is refused with a text that names
//! the locators this target takes. Pure: the one OS read (does the target's
//! window have a raw-CDP endpoint) is done by the caller and passed in.

use synapse_core::{ElementId, error_codes};

use super::{TargetActParams, target_act_coordinate, target_act_has_key_chord};
use crate::server::SessionTarget;

/// A DOM-style verb of `act`: the verbs that used to fall through to the
/// Chrome bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActVerb {
    Click,
    Dblclick,
    /// `press` of a named element (a `press` with a key chord is a key press).
    Press,
    Check,
    Uncheck,
    Select,
    Focus,
    Blur,
    Clear,
    SelectText,
    Submit,
    DispatchEvent,
}

impl ActVerb {
    /// The verb of `params`, or `None` for a verb this route does not decide.
    pub(super) fn of(params: &TargetActParams) -> Option<Self> {
        Some(match params.verb.as_str() {
            "click" => Self::Click,
            "dblclick" => Self::Dblclick,
            "press" if !target_act_has_key_chord(params) => Self::Press,
            "check" => Self::Check,
            "uncheck" => Self::Uncheck,
            "select" => Self::Select,
            "focus" => Self::Focus,
            "blur" => Self::Blur,
            "clear" => Self::Clear,
            "select_text" | "selecttext" => Self::SelectText,
            "submit" => Self::Submit,
            "dispatch_event" | "dispatchevent" => Self::DispatchEvent,
            _ => return None,
        })
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Dblclick => "dblclick",
            Self::Press => "press",
            Self::Check => "check",
            Self::Uncheck => "uncheck",
            Self::Select => "select",
            Self::Focus => "focus",
            Self::Blur => "blur",
            Self::Clear => "clear",
            Self::SelectText => "select_text",
            Self::Submit => "submit",
            Self::DispatchEvent => "dispatch_event",
        }
    }
}

/// The kind of the session target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActTarget {
    None,
    NativeWindow,
    /// A native window (WebView2 host) whose page has a raw CDP endpoint.
    NativeWindowWithRawCdp,
    /// #79: a native window that hosts a WebView2 page with no raw CDP
    /// endpoint and is not in front. UIA input to its page takes the
    /// foreground, so a click there is refused.
    WebViewBehindWithoutCdp,
    RawCdpPage,
    /// A CDP target without a raw endpoint: served by the Chrome extension bridge.
    BridgeTab,
}

impl ActTarget {
    /// `raw_cdp_endpoint`: whether `synapse_a11y::endpoint_for_window` found a
    /// raw CDP endpoint for the target's window; `webview_behind`: whether that
    /// window hosts a WebView2 page and is not the foreground window (the
    /// caller's OS reads).
    pub(super) const fn of(
        target: Option<&SessionTarget>,
        raw_cdp_endpoint: bool,
        webview_behind: bool,
    ) -> Self {
        match (target, raw_cdp_endpoint) {
            (None, _) => Self::None,
            (Some(SessionTarget::Window { .. }), false) if webview_behind => {
                Self::WebViewBehindWithoutCdp
            }
            (Some(SessionTarget::Window { .. }), false) => Self::NativeWindow,
            (Some(SessionTarget::Window { .. }), true) => Self::NativeWindowWithRawCdp,
            (Some(SessionTarget::Cdp { .. }), true) => Self::RawCdpPage,
            (Some(SessionTarget::Cdp { .. }), false) => Self::BridgeTab,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::None => "no target",
            Self::NativeWindow => "a native window",
            Self::NativeWindowWithRawCdp => "a native window hosting a raw-CDP page",
            Self::WebViewBehindWithoutCdp => {
                "a WebView2 window behind another window with no raw CDP endpoint"
            }
            Self::RawCdpPage => "a raw-CDP page",
            Self::BridgeTab => "a Chrome bridge tab",
        }
    }
}

/// The kind of locator a call gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActLocator {
    /// A UIA element_id from find.
    NativeElementId,
    /// A web element_id from find (it carries a CDP backend node).
    CdpElementId,
    /// An element_id that is not a Synapse id: a plain DOM id.
    PageElementId,
    NameAndRole,
    NameAlone,
    /// A CSS selector, or a match only a web page resolves (role alone, value,
    /// option).
    Selector,
    /// x/y coordinates.
    Point,
    None,
}

impl ActLocator {
    /// The locator `params` gives, in the order the router reads it: an
    /// element_id first, then a selector or another page-only match (for
    /// `select`, `value` and `option` name the option, not the element), then
    /// name + role, name alone, x/y.
    pub(super) fn of(params: &TargetActParams) -> Self {
        let given = |value: Option<&String>| value.is_some_and(|value| !value.trim().is_empty());
        if let Some(raw) = params
            .element_id
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
        {
            return match ElementId::parse(raw) {
                Ok(id) if synapse_a11y::cdp_backend_from_element_id(&id).is_some() => {
                    Self::CdpElementId
                }
                Ok(_) => Self::NativeElementId,
                Err(_) => Self::PageElementId,
            };
        }
        let value_names_element = params.verb.as_str() != "select"
            && (given(params.value.as_ref()) || given(params.option.as_ref()));
        let name = given(params.name.as_ref());
        let role = given(params.role.as_ref());
        if given(params.selector.as_ref()) || value_names_element || (role && !name) {
            Self::Selector
        } else if name && role {
            Self::NameAndRole
        } else if name {
            Self::NameAlone
        } else if matches!(target_act_coordinate(params), Ok(Some(_))) {
            Self::Point
        } else {
            Self::None
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::NativeElementId => "a native element_id from find",
            Self::CdpElementId => "a web element_id from find",
            Self::PageElementId => "a plain DOM id",
            Self::NameAndRole => "name + role",
            Self::NameAlone => "name",
            Self::Selector => "a selector (or a role / value / option match)",
            Self::Point => "x/y",
            Self::None => "no locator",
        }
    }
}

/// Which backend serves a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ActBackend {
    NativeAccessibility,
    RawCdp,
    /// A Chrome bridge tab: the router keeps its path from before #2348.
    ChromeBridge,
    /// Nothing is sent; `code` is a `synapse_core::error_codes` code.
    Refused {
        code: &'static str,
        text: String,
    },
}

/// One `act` call as the router sees it: verb, session target kind, locator
/// kind, and whether an option was given (for `select`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ActRoute {
    verb: ActVerb,
    target: ActTarget,
    locator: ActLocator,
    option_given: bool,
}

impl ActRoute {
    pub(super) const fn new(
        verb: ActVerb,
        target: ActTarget,
        locator: ActLocator,
        option_given: bool,
    ) -> Self {
        Self {
            verb,
            target,
            locator,
            option_given,
        }
    }

    /// The backend that serves this call, or the refusal (nothing is sent).
    pub(super) fn backend(&self) -> ActBackend {
        let verb = self.verb.name();
        if self.target == ActTarget::BridgeTab {
            return ActBackend::ChromeBridge;
        }
        // #79: no route reaches the page of a WebView2 window behind another
        // window without a raw CDP endpoint; UIA Invoke would take the foreground.
        if self.target == ActTarget::WebViewBehindWithoutCdp && self.verb == ActVerb::Click {
            return ActBackend::Refused {
                code: error_codes::A11Y_CDP_UNREACHABLE,
                text: format!(
                    "target_act verb={verb} on {}: a click there would take the foreground; nothing was sent. Launch the app through Synapse process launch with cdp_debug=true, then use a CDP element or target-scoped coordinate click",
                    self.target.name()
                ),
            };
        }
        if self.locator == ActLocator::None {
            return ActBackend::Refused {
                code: error_codes::TOOL_PARAMS_INVALID,
                text: format!(
                    "target_act verb={verb} needs a locator; on {} it takes {}",
                    self.target.name(),
                    self.accepted()
                ),
            };
        }
        if self.verb == ActVerb::Select && !self.option_given {
            return ActBackend::Refused {
                code: error_codes::TOOL_PARAMS_INVALID,
                text: "target_act verb=select: an option is needed (option, option_label, value or option_index); nothing was sent".to_owned(),
            };
        }
        let (native, raw_cdp) = self.served();
        if native.contains(&self.locator) {
            return ActBackend::NativeAccessibility;
        }
        if raw_cdp.contains(&self.locator) {
            return ActBackend::RawCdp;
        }
        let (code, text) = if self.target == ActTarget::None {
            (
                error_codes::TARGET_NOT_SET,
                format!(
                    "target_act verb={verb} requires this MCP session to own a target; set one with set_target (target operation=set) or cdp_open_tab and claim it (target operation=claim) first; refusing to use the human foreground tab"
                ),
            )
        } else if self.locator == ActLocator::Point {
            (
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act verb={verb} does not accept x/y; use verb=click or verb=type for coordinate fallback"
                ),
            )
        } else {
            (
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb={verb} on {} takes {}; got {}; nothing was sent",
                    self.target.name(),
                    self.accepted(),
                    self.locator.name()
                ),
            )
        };
        ActBackend::Refused { code, text }
    }

    /// The locators this verb takes on this target: (served by native
    /// accessibility, served by raw CDP). A bridge tab is not listed: it keeps
    /// its path from before #2348.
    const fn served(&self) -> (&'static [ActLocator], &'static [ActLocator]) {
        use ActLocator::{
            CdpElementId, NameAlone, NameAndRole, NativeElementId, PageElementId, Point, Selector,
        };
        use ActVerb::{
            Blur, Check, Clear, Click, Dblclick, Focus, Press, Select, SelectText, Uncheck,
        };
        // A click on `WebViewBehindWithoutCdp` is refused in `backend`; its
        // other verbs keep the native window's locators.
        match (self.target, self.verb) {
            (
                ActTarget::NativeWindow
                | ActTarget::NativeWindowWithRawCdp
                | ActTarget::WebViewBehindWithoutCdp,
                Click,
            ) => (
                &[NativeElementId, NameAndRole, NameAlone, Point],
                &[CdpElementId],
            ),
            (
                ActTarget::NativeWindow
                | ActTarget::NativeWindowWithRawCdp
                | ActTarget::WebViewBehindWithoutCdp,
                Dblclick,
            ) => (&[NativeElementId, Point], &[CdpElementId]),
            (
                ActTarget::NativeWindow
                | ActTarget::NativeWindowWithRawCdp
                | ActTarget::WebViewBehindWithoutCdp,
                Check | Uncheck,
            )
            | (ActTarget::NativeWindow | ActTarget::WebViewBehindWithoutCdp, Select) => {
                (&[NativeElementId, NameAndRole, NameAlone], &[])
            }
            (ActTarget::NativeWindowWithRawCdp, Select) => {
                (&[NativeElementId, NameAndRole, NameAlone], &[CdpElementId])
            }
            (ActTarget::RawCdpPage, Click | Dblclick) => (
                &[NativeElementId],
                &[CdpElementId, Selector, NameAndRole, NameAlone, Point],
            ),
            (ActTarget::RawCdpPage, Press | Select) => (
                &[],
                &[
                    CdpElementId,
                    PageElementId,
                    Selector,
                    NameAndRole,
                    NameAlone,
                ],
            ),
            (ActTarget::RawCdpPage, Focus | Blur | Clear | SelectText) => (&[], &[CdpElementId]),
            // #2406: no target serves no click / dblclick by element id: the
            // refusal below answers TARGET_NOT_SET.
            _ => (&[], &[]),
        }
    }

    /// The locators this verb takes on this target, as text. A verb this
    /// target does not serve at all names what verb=click takes there.
    fn accepted(&self) -> String {
        let (native, raw_cdp) = self.served();
        if native.is_empty() && raw_cdp.is_empty() && self.verb == ActVerb::Click {
            // #2406: no target serves no click at all.
            return "nothing".to_owned();
        }
        if native.is_empty() && raw_cdp.is_empty() {
            let click = Self::new(ActVerb::Click, self.target, self.locator, self.option_given);
            return format!(
                "nothing (verb={} is not available on {}); verb=click there takes {}",
                self.verb.name(),
                self.target.name(),
                click.accepted()
            );
        }
        native
            .iter()
            .chain(raw_cdp)
            .map(|locator| locator.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
#[path = "act_route_table_2348.rs"]
mod act_route_table_2348;
