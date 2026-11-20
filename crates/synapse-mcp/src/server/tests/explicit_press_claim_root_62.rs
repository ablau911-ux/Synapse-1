//! #62 explicit native press: exact ownership and stable root before delivery.
//! NEEDS-SEAM in server/m2_tools.rs, used by try_act_press_background_target:
//! fn dispatch_explicit_press_for_session_with(
//!   &self, requested_hwnd: i64, session_id: Option<&str>,
//!   resolve_root: impl FnMut(i64) -> Result<i64, rmcp::ErrorData>,
//!   deliver: impl FnOnce(i64) -> Result<(), rmcp::ErrorData>,
//! ) -> Result<(), rmcp::ErrorData>
//! Resolve root, check real claim registry (peer conflict first), resolve again,
//! refuse changed/invalid root with ACTION_TARGET_INVALID before calling deliver.
//! Deliver receives exactly checked root. Production must use this boundary
//! immediately before physical key posting, with all posts bound to that root.
#![cfg(windows)]
use crate::server::g4_unit_support::{
    claim_window, error_code, isolated_service, seed_live_session,
};
use rmcp::ErrorData;
use std::cell::{Cell, RefCell};
use synapse_core::error_codes;

const SESSION: &str = "fu3-62";
const PEER: &str = "fu3-62-peer";
const REQUESTED: i64 = 0x6201;
const ROOT: i64 = 0x6200;
const SWAPPED: i64 = 0x6300;

fn dispatch(
    service: &crate::server::SynapseService,
    roots: &[i64],
    sent: &RefCell<Vec<i64>>,
) -> Result<(), ErrorData> {
    assert!(sent.borrow().is_empty(), "precondition: zero input");
    let reads = Cell::new(0);
    service.dispatch_explicit_press_for_session_with(
        REQUESTED,
        Some(SESSION),
        |hwnd| {
            assert_eq!(hwnd, REQUESTED, "recheck original target, never foreground");
            let index = reads.get();
            reads.set(index + 1);
            Ok(roots[index.min(roots.len() - 1)])
        },
        |hwnd| {
            sent.borrow_mut().push(hwnd);
            Ok(())
        },
    )
}

fn assert_refused(result: Result<(), ErrorData>, code: &str, sent: &RefCell<Vec<i64>>) {
    let error = result.expect_err("press must be refused");
    assert_eq!(error_code(&error).as_deref(), Some(code), "{error:?}");
    assert_eq!(*sent.borrow(), Vec::<i64>::new(), "no physical input");
}

#[test]
fn must_pass_owned_child_delivers_once_to_exact_checked_root() {
    let (_home, service) = isolated_service();
    seed_live_session(&service, SESSION);
    claim_window(&service, SESSION, ROOT);
    let sent = RefCell::new(Vec::new());
    dispatch(&service, &[ROOT, ROOT], &sent).expect("stable owned root");
    assert_eq!(*sent.borrow(), vec![ROOT], "one delivery to checked root");
}

#[test]
fn must_fail_unclaimed_native_press_sends_no_input() {
    let (_home, service) = isolated_service();
    seed_live_session(&service, SESSION);
    assert_eq!(
        service
            .target_claim_for_session(
                SESSION,
                &crate::server::SessionTarget::Window { hwnd: ROOT }
            )
            .expect("readback")
            .map(|claim| claim.owner_session_id),
        None
    );
    let sent = RefCell::new(Vec::new());
    assert_refused(
        dispatch(&service, &[ROOT, ROOT], &sent),
        error_codes::TARGET_CLAIM_NOT_FOUND,
        &sent,
    );
}

#[test]
fn must_fail_claim_on_another_window_does_not_authorize_press() {
    let (_home, service) = isolated_service();
    seed_live_session(&service, SESSION);
    claim_window(&service, SESSION, SWAPPED);
    let sent = RefCell::new(Vec::new());
    assert_refused(
        dispatch(&service, &[ROOT, ROOT], &sent),
        error_codes::TARGET_CLAIM_NOT_FOUND,
        &sent,
    );
}

#[test]
fn must_fail_peer_claim_keeps_conflict_code_and_zero_delivery() {
    let (_home, service) = isolated_service();
    seed_live_session(&service, SESSION);
    seed_live_session(&service, PEER);
    claim_window(&service, PEER, ROOT);
    let sent = RefCell::new(Vec::new());
    assert_refused(
        dispatch(&service, &[ROOT, ROOT], &sent),
        error_codes::TARGET_CO_OWNED,
        &sent,
    );
}

#[test]
fn must_fail_root_swap_after_ownership_check_sends_no_input() {
    let (_home, service) = isolated_service();
    seed_live_session(&service, SESSION);
    claim_window(&service, SESSION, ROOT);
    // Even owning BOTH roots cannot authorize changing the checked destination.
    claim_window(&service, SESSION, SWAPPED);
    let sent = RefCell::new(Vec::new());
    assert_refused(
        dispatch(&service, &[ROOT, SWAPPED], &sent),
        error_codes::ACTION_TARGET_INVALID,
        &sent,
    );
}

#[test]
fn must_fail_disappeared_root_after_check_sends_no_input() {
    let (_home, service) = isolated_service();
    seed_live_session(&service, SESSION);
    claim_window(&service, SESSION, ROOT);
    let sent = RefCell::new(Vec::new());
    assert_refused(
        dispatch(&service, &[ROOT, 0], &sent),
        error_codes::ACTION_TARGET_INVALID,
        &sent,
    );
}

#[test]
fn must_fail_explicit_press_entrypoint_bypassing_bound_dispatch() {
    // Last-resort wiring guard: seam-only green cannot prove handler coverage.
    let source = include_str!("../m2_tools.rs");
    let route = source
        .split("async fn try_act_press_background_target(")
        .nth(1)
        .expect("production route")
        .split("async fn try_act_keymap_background_target(")
        .next()
        .expect("route boundary");
    assert!(
        route.contains("dispatch_explicit_press_for_session_with("),
        "real explicit-press route must use tested claim/root/delivery boundary"
    );
}
