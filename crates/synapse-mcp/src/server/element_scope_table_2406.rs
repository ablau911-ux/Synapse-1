//! FROZEN behaviour table of Gitea #2406 (owner safety, review findings 1
//! and 2): rows E1-E11 (`ElementScope`) and the two route rows RT1, RT2
//! (`ActRoute`, no target + click / dblclick by element id).
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/server/element_scope_table_2406.rs
//! (registered by the `#[cfg(test)] #[path]` lines at the end of
//! element_scope.rs).
//! One test per row, named after the row. CONTROL cases: every allowing row
//! (E3, E5, E8, E10's first case) carries a refusing neighbour, and every
//! refusing row (E1, E2, E4, E6, E7, E9, E10's second case, E11) carries an
//! allowing neighbour, so a stub that allows everything AND a stub that
//! refuses everything with one code both fail every row. RT1/RT2 carry the
//! control "a native window target keeps native accessibility".

use synapse_core::error_codes;

use super::super::act_route::{ActBackend, ActLocator, ActRoute, ActTarget, ActVerb};
use super::{ElementScope, ScopeElement, ScopeTarget, ScopeVerdict};
use crate::server::SessionTarget;

/// Window W1 (the target), window W2 (another window), a child HWND inside
/// each (a native element's own HWND differs from its root).
const W1: i64 = 0x1_0010;
const W2: i64 = 0x2_0020;
const W1_CHILD: i64 = 0x1_0011;
const W2_CHILD: i64 = 0x2_0021;
/// DevTools page ids (32 hex like Chromium's); F1 is a frame target under P1.
const P1: &str = "A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1";
const P2: &str = "B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2";
const F1: &str = "C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3";
const VERB: &str = "click";

fn window(hwnd: i64) -> Option<SessionTarget> {
    Some(SessionTarget::Window { hwnd })
}

fn page_target(window_hwnd: i64, page: &str) -> Option<SessionTarget> {
    Some(SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id: page.to_owned(),
    })
}

/// A UIA element id whose own HWND is `element_hwnd`; `root` is its
/// top-level window as `ElementRoot` would read it.
fn native(element_hwnd: i64, root: i64) -> ScopeElement {
    ScopeElement::new(
        &format!("0x{element_hwnd:x}:2a00000001"),
        Some(root),
        Vec::new(),
    )
}

/// A web element id from find: window `hwnd`, page `page`, node 42;
/// `frames`: the frame targets attached under the target page.
fn web(hwnd: i64, page: &str, frames: &[&str]) -> ScopeElement {
    let id = synapse_a11y::cdp_element_id_for_target(hwnd, page, 42);
    ScopeElement::new(
        id.as_str(),
        None,
        frames.iter().map(|frame| (*frame).to_owned()).collect(),
    )
}

fn verdict(session: Option<SessionTarget>, pages: &[&str], element: ScopeElement) -> ScopeVerdict {
    let target = ScopeTarget::new(
        session,
        pages.iter().map(|page| (*page).to_owned()).collect(),
    );
    ElementScope::new(target, element).verdict(VERB)
}

/// The refusal (code, text), or a failure naming what came instead.
fn refusal(verdict: ScopeVerdict, case: &str) -> Result<(&'static str, String), String> {
    match verdict {
        ScopeVerdict::Refused { code, text } => Ok((code, text)),
        ScopeVerdict::Allowed => Err(format!("{case}: expected a refusal, got Allowed")),
    }
}

#[test]
fn e1_no_target_native_element_of_any_window_is_target_not_set() -> Result<(), String> {
    for (element_hwnd, root) in [(W1_CHILD, W1), (W2_CHILD, W2)] {
        let (code, text) = refusal(verdict(None, &[], native(element_hwnd, root)), "E1")?;
        assert_eq!(
            code,
            error_codes::TARGET_NOT_SET,
            "E1 root 0x{root:x}: {text}"
        );
        assert!(text.contains("set_target"), "E1: {text}");
    }
    // CONTROL: the same element with its own window as target is allowed.
    assert_eq!(
        verdict(window(W2), &[], native(W2_CHILD, W2)),
        ScopeVerdict::Allowed
    );
    Ok(())
}

#[test]
fn e2_no_target_page_element_is_target_not_set() -> Result<(), String> {
    let (code, text) = refusal(verdict(None, &[], web(W1, P1, &[])), "E2")?;
    assert_eq!(code, error_codes::TARGET_NOT_SET, "E2: {text}");
    // CONTROL: the same element with its page as target is allowed.
    assert_eq!(
        verdict(page_target(W1, P1), &[], web(W1, P1, &[])),
        ScopeVerdict::Allowed
    );
    Ok(())
}

#[test]
fn e3_window_target_native_element_inside_it_is_allowed() {
    assert_eq!(
        verdict(window(W1), &[], native(W1_CHILD, W1)),
        ScopeVerdict::Allowed
    );
    // CONTROL: an element of W2 is not allowed (row E4).
    assert_ne!(
        verdict(window(W1), &[], native(W2_CHILD, W2)),
        ScopeVerdict::Allowed
    );
}

#[test]
fn e4_window_target_native_element_of_another_window_is_refused() -> Result<(), String> {
    let (code, text) = refusal(verdict(window(W1), &[], native(W2_CHILD, W2)), "E4")?;
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "E4: {text}");
    assert!(text.contains("another window"), "E4: {text}");
    // The object has no window title as input, so no title can be repeated.
    // CONTROL: an element of W1 is allowed (row E3).
    assert_eq!(
        verdict(window(W1), &[], native(W1_CHILD, W1)),
        ScopeVerdict::Allowed
    );
    Ok(())
}

#[test]
fn e5_window_showing_one_page_page_element_on_it_is_allowed() {
    assert_eq!(
        verdict(window(W1), &[P1], web(W1, P1, &[])),
        ScopeVerdict::Allowed
    );
    // CONTROL: an element whose id names P2 is not allowed (row E6).
    assert_ne!(
        verdict(window(W1), &[P1], web(W1, P2, &[])),
        ScopeVerdict::Allowed
    );
}

#[test]
fn e6_window_showing_one_page_element_id_naming_another_page_is_refused() -> Result<(), String> {
    let (code, text) = refusal(verdict(window(W1), &[P1], web(W1, P2, &[])), "E6")?;
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "E6: {text}");
    // CONTROL: the element on P1 is allowed (row E5).
    assert_eq!(
        verdict(window(W1), &[P1], web(W1, P1, &[])),
        ScopeVerdict::Allowed
    );
    Ok(())
}

#[test]
fn e7_window_whose_browser_lists_two_pages_refuses_any_page_element() -> Result<(), String> {
    for (case, element) in [
        ("E7 on P1", web(W1, P1, &[])),
        ("E7 on P2", web(W1, P2, &[])),
    ] {
        let (code, text) = refusal(verdict(window(W1), &[P1, P2], element), case)?;
        assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "{case}: {text}");
        assert!(text.contains("page"), "{case}: {text}");
    }
    // CONTROL: the same element when the window lists only P1 is allowed.
    assert_eq!(
        verdict(window(W1), &[P1], web(W1, P1, &[])),
        ScopeVerdict::Allowed
    );
    Ok(())
}

#[test]
fn e8_page_target_element_on_the_page_or_on_a_frame_of_it_is_allowed() {
    assert_eq!(
        verdict(page_target(W1, P1), &[], web(W1, P1, &[])),
        ScopeVerdict::Allowed
    );
    assert_eq!(
        verdict(page_target(W1, P1), &[], web(W1, F1, &[F1])),
        ScopeVerdict::Allowed
    );
    // CONTROL: the same page F1 when it is not a frame under P1 is refused.
    assert_ne!(
        verdict(page_target(W1, P1), &[], web(W1, F1, &[])),
        ScopeVerdict::Allowed
    );
}

#[test]
fn e9_page_target_element_on_another_page_is_refused() -> Result<(), String> {
    let (code, text) = refusal(verdict(page_target(W1, P1), &[], web(W1, P2, &[F1])), "E9")?;
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "E9: {text}");
    // CONTROL: the element on P1 is allowed (row E8).
    assert_eq!(
        verdict(page_target(W1, P1), &[], web(W1, P1, &[])),
        ScopeVerdict::Allowed
    );
    Ok(())
}

#[test]
fn e10_page_target_native_element_inside_its_window_allowed_inside_another_refused()
-> Result<(), String> {
    assert_eq!(
        verdict(page_target(W1, P1), &[], native(W1_CHILD, W1)),
        ScopeVerdict::Allowed
    );
    let (code, text) = refusal(
        verdict(page_target(W1, P1), &[], native(W2_CHILD, W2)),
        "E10",
    )?;
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "E10: {text}");
    Ok(())
}

#[test]
fn e11_unreadable_element_id_is_refused_before_anything_else() -> Result<(), String> {
    // Before anything else: also before the target check (no target) and
    // whatever was read about the element.
    for (case, session) in [("E11 window W1", window(W1)), ("E11 no target", None)] {
        let element = ScopeElement::new("not-an-element-id", Some(W1), Vec::new());
        let (code, text) = refusal(verdict(session, &[P1], element), case)?;
        assert_eq!(code, error_codes::TOOL_PARAMS_INVALID, "{case}: {text}");
    }
    // CONTROL: a readable id of W1 is allowed.
    assert_eq!(
        verdict(window(W1), &[P1], native(W1_CHILD, W1)),
        ScopeVerdict::Allowed
    );
    Ok(())
}

/// The route's refusal code, or a failure naming the backend that came instead.
fn route_code(
    verb: ActVerb,
    target: ActTarget,
    locator: ActLocator,
) -> Result<&'static str, String> {
    match ActRoute::new(verb, target, locator, false).backend() {
        ActBackend::Refused { code, .. } => Ok(code),
        other => Err(format!(
            "{verb:?} on {target:?} with {locator:?}: expected a refusal, got {other:?}"
        )),
    }
}

#[test]
fn rt1_route_no_target_click_by_element_id_is_target_not_set() -> Result<(), String> {
    for locator in [ActLocator::NativeElementId, ActLocator::CdpElementId] {
        let code = route_code(ActVerb::Click, ActTarget::None, locator)?;
        assert_eq!(code, error_codes::TARGET_NOT_SET, "RT1 {locator:?}");
    }
    // CONTROL: a native window target keeps native accessibility.
    assert_eq!(
        ActRoute::new(
            ActVerb::Click,
            ActTarget::NativeWindow,
            ActLocator::NativeElementId,
            false
        )
        .backend(),
        ActBackend::NativeAccessibility
    );
    Ok(())
}

#[test]
fn rt2_route_no_target_dblclick_by_element_id_is_target_not_set() -> Result<(), String> {
    for locator in [ActLocator::NativeElementId, ActLocator::CdpElementId] {
        let code = route_code(ActVerb::Dblclick, ActTarget::None, locator)?;
        assert_eq!(code, error_codes::TARGET_NOT_SET, "RT2 {locator:?}");
    }
    // CONTROL: a native window target keeps native accessibility.
    assert_eq!(
        ActRoute::new(
            ActVerb::Dblclick,
            ActTarget::NativeWindow,
            ActLocator::NativeElementId,
            false
        )
        .backend(),
        ActBackend::NativeAccessibility
    );
    Ok(())
}
