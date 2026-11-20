//! #202: `act_stroke` refuses before dispatch when a live PEER session holds a
//! claim on what the decided route actually touches: the session CDP tab for
//! the CDP route, every window under the planned path for the screen route.
//! The existing `stroke_claim_targets_202` tests stop at the target list; these
//! run the list through the real claim registry of a real service (live peer
//! session, real claim rows, real windows for root canonicalisation).
//!
//! NEEDS-SEAM (fixer extracts the claim block of `act_stroke` after
//! `act_stroke_route_for_request`, behaviour unchanged):
//!   fn ensure_act_stroke_claims_allow(
//!       &self,
//!       session_id: &str,
//!       route: &ActStrokeRoute,
//!       plan: &crate::m2::ActStrokePlan,
//!       root_at: impl Fn(synapse_core::Point) -> Option<i64>,
//!   ) -> Result<(), ErrorData>
//! (non-empty target list: `ensure_target_claim_allows_session` per target;
//! empty list: the current session-target / foreground fallback).

#![cfg(windows)]

use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

use super::*;
use crate::server::SynapseService;
use crate::server::session_registry::unix_time_ms_now;
use crate::test_support::{PumpedWindows, from_hwnd};

const ME: &str = "stroke-202-me";
const PEER: &str = "stroke-202-peer";

/// Hidden, never-activated top-level window owned by this test, on a
/// message-pumping UI thread (a concurrent UIA query that reaches it is
/// answered); destroyed on drop.
struct OwnWindow(PumpedWindows);

impl OwnWindow {
    fn hwnd(&self) -> i64 {
        self.0.hwnd(0)
    }
}

fn own_window() -> OwnWindow {
    let ui = PumpedWindows::spawn(|| {
        // SAFETY: STATIC is a system class; the window is never shown.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("synapse #202 stroke claim"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
        }?;
        Ok(vec![from_hwnd(hwnd)])
    });
    OwnWindow(ui.unwrap_or_else(|error| panic!("CreateWindowExW failed: {error:#}")))
}

fn seed_live_session(service: &SynapseService, session_id: &str) {
    use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
    let state = rmcp::transport::streamable_http_server::session::SessionState::new(
        InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("claude-code", "0.0.0-test"),
        ),
    );
    assert!(
        service
            .session_registry_ref()
            .lock()
            .expect("session registry")
            .record_initialized(session_id, &state, "http", unix_time_ms_now()),
        "precondition: {session_id} registered"
    );
}

fn service_with_peer_claim(claim: SessionTarget) -> (tempfile::TempDir, SynapseService) {
    let root = tempfile::tempdir().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(root.path()).expect("service");
    seed_live_session(&service, ME);
    seed_live_session(&service, PEER);
    let now = unix_time_ms_now();
    let live = std::collections::BTreeSet::from([ME.to_owned(), PEER.to_owned()]);
    service
        .lock_target_claims()
        .expect("claims lock")
        .claim(PEER, claim, 60_000, now, &live)
        .unwrap_or_else(|_conflict| panic!("precondition: peer claim accepted"));
    assert_eq!(
        peer_claims(&service),
        1,
        "precondition: one live peer claim"
    );
    (root, service)
}

fn peer_claims(service: &SynapseService) -> usize {
    let now = unix_time_ms_now();
    service
        .lock_target_claims()
        .expect("claims lock")
        .reads(now)
        .into_iter()
        .filter(|claim| claim.owner_session_id == PEER && claim.expires_at_unix_ms > now)
        .count()
}

fn line_plan() -> crate::m2::ActStrokePlan {
    let params: crate::m2::ActStrokeParams = serde_json::from_value(serde_json::json!({
        "path": {"kind": "line", "from": {"x": 10.0, "y": 10.0}, "to": {"x": 90.0, "y": 10.0}},
        "duration_or_speed": {"kind": "duration_ms", "duration_ms": 100}
    }))
    .expect("stroke params");
    crate::m2::validate_act_stroke_params(&params).expect("stroke plan")
}

fn tab(window: &OwnWindow, id: &str) -> SessionTarget {
    SessionTarget::Cdp {
        window_hwnd: window.hwnd(),
        cdp_target_id: id.to_owned(),
    }
}

fn code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()?
        .get("code")
        .and_then(serde_json::Value::as_str)
}

#[test]
fn must_fail_stroke_into_a_peer_claimed_session_tab_is_refused_before_dispatch() {
    let browser = own_window();
    let (_root, service) = service_with_peer_claim(tab(&browser, "TAB-T"));
    let route = act_stroke_route(Some(tab(&browser, "TAB-T")), &line_plan(), false);
    let error = service
        .ensure_act_stroke_claims_allow(ME, &route, &line_plan(), |_point| None)
        .expect_err("the peer's tab is claimed");
    assert_eq!(code(&error), Some(error_codes::TARGET_CO_OWNED));
    assert_eq!(peer_claims(&service), 1, "the refusal changes no claim");
}

#[test]
fn must_pass_cdp_routed_stroke_ignores_screen_windows_under_its_page_path() {
    let browser = own_window();
    let peer_window = own_window();
    let (_root, service) = service_with_peer_claim(SessionTarget::Window {
        hwnd: peer_window.hwnd(),
    });
    let route = act_stroke_route(Some(tab(&browser, "TAB-M")), &line_plan(), false);
    let peer = peer_window.hwnd();
    let result =
        service.ensure_act_stroke_claims_allow(ME, &route, &line_plan(), |_point| Some(peer));
    assert!(
        matches!(result, Ok(())),
        "page coordinates are not screen pixels: {result:?}"
    );
}

#[test]
fn must_fail_screen_stroke_across_a_peer_claimed_window_is_refused() {
    let peer_window = own_window();
    let (_root, service) = service_with_peer_claim(SessionTarget::Window {
        hwnd: peer_window.hwnd(),
    });
    let route = act_stroke_route(None, &line_plan(), false);
    assert_eq!(route, ActStrokeRoute::Screen, "precondition: screen route");
    let peer = peer_window.hwnd();
    let error = service
        .ensure_act_stroke_claims_allow(ME, &route, &line_plan(), |point| {
            (point.x >= 50).then_some(peer)
        })
        .expect_err("the path ends in the peer's window");
    assert_eq!(code(&error), Some(error_codes::TARGET_CO_OWNED));
    assert_eq!(peer_claims(&service), 1);
}

#[test]
fn must_pass_screen_stroke_over_unclaimed_windows_is_allowed() {
    let peer_window = own_window();
    let free_window = own_window();
    let (_root, service) = service_with_peer_claim(SessionTarget::Window {
        hwnd: peer_window.hwnd(),
    });
    let route = act_stroke_route(None, &line_plan(), false);
    let free = free_window.hwnd();
    let result =
        service.ensure_act_stroke_claims_allow(ME, &route, &line_plan(), |_point| Some(free));
    assert!(matches!(result, Ok(())), "{result:?}");
}

#[test]
fn must_pass_act_stroke_uses_the_claim_seam() {
    let source = include_str!("../m2_tools.rs");
    let start = source.find("pub async fn act_stroke(").expect("handler");
    let body = &source[start..];
    let body = &body[..body.find("\n    #[tool(").unwrap_or(body.len())];
    assert!(body.contains("ensure_act_stroke_claims_allow("));
    let claim = body
        .find("ensure_act_stroke_claims_allow(")
        .unwrap_or(usize::MAX);
    let dispatch = body
        .find("try_act_stroke_cdp_background_target(")
        .expect("CDP dispatch present");
    assert!(claim < dispatch, "claims are checked before any dispatch");
}
