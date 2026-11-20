//! #6: an explicitly authorized focus recovery (this session holds an exact
//! live claim on the requested window) must pass the action gate when the OS
//! has no foreground window; every request without that exact proof stays
//! fail-closed with `A11Y_NO_FOREGROUND`.
//!
//! Contract: issue #6 "Expected"; server/context.rs
//! `ensure_supported_use_allows_action_scoped` doc ("an exactly claimed
//! focus-recovery target (#6)"); server/m2_tools.rs `act_focus_window` handler,
//! which computes the proof with `focus_recovery_claim_proven` and passes it to
//! the gate. The m2 half (`before_foreground_for_focus`) is covered by
//! m2/tests/focus_window_no_foreground_6.rs.
//!
//! Drives the handler's proof + the real gate on an isolated service with the
//! real claim registry. Seam (NEEDS-SEAM, see G4 report):
//! `SynapseService::focus_recovery_claim_proven_for(&self, target_hwnd:
//! Option<i64>, session_id: Option<&str>) -> bool` — the body of
//! `focus_recovery_claim_proven` after the `RequestContext` -> session-id and
//! params -> HWND reads; the handler must call it.

use synapse_core::error_codes;

use crate::server::g4_unit_support::{
    claim_window, error_code, force_no_foreground, isolated_service, seed_live_session,
};

const TARGET: i64 = 0x0007_a010;
const SESSION: &str = "g4-6-session";
const PEER: &str = "g4-6-peer";

fn gate_code(service: &crate::server::SynapseService, proven: bool) -> Option<String> {
    service
        .ensure_supported_use_allows_action_scoped("act_focus_window", None, proven)
        .err()
        .as_ref()
        .and_then(error_code)
}

#[test]
fn must_pass_claimed_focus_recovery_passes_the_gate_with_no_foreground() {
    let (_root, service) = isolated_service();
    assert_eq!(
        service.allow_unknown_profile().ok(),
        Some(true),
        "precondition"
    );
    seed_live_session(&service, SESSION);
    claim_window(&service, SESSION, TARGET);
    force_no_foreground(&service);

    let proven = service.focus_recovery_claim_proven_for(Some(TARGET), Some(SESSION));
    assert!(proven, "an exact live claim is the recovery proof");
    let preflight = service
        .ensure_supported_use_allows_action_scoped("act_focus_window", None, proven)
        .expect("authorized recovery must pass the gate with no foreground");
    assert_eq!(preflight.tool, "act_focus_window");
    assert_eq!(preflight.status, "no_foreground_scope_evaluated");
    assert_eq!(
        preflight.before.hwnd, 0,
        "no prior foreground is recorded as hwnd 0"
    );
}

#[test]
fn must_fail_unclaimed_focus_target_stays_fail_closed() {
    let (_root, service) = isolated_service();
    seed_live_session(&service, SESSION);
    force_no_foreground(&service);

    let proven = service.focus_recovery_claim_proven_for(Some(TARGET), Some(SESSION));
    assert!(!proven, "no claim, no proof");
    assert_eq!(
        gate_code(&service, proven).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
}

#[test]
fn must_fail_a_peer_sessions_claim_is_not_this_sessions_proof() {
    let (_root, service) = isolated_service();
    seed_live_session(&service, SESSION);
    seed_live_session(&service, PEER);
    claim_window(&service, PEER, TARGET);
    force_no_foreground(&service);

    let proven = service.focus_recovery_claim_proven_for(Some(TARGET), Some(SESSION));
    assert!(
        !proven,
        "a peer's claim never proves this session's recovery"
    );
    assert_eq!(
        gate_code(&service, proven).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
}

#[test]
fn must_fail_a_claim_on_a_different_window_is_not_proof() {
    let (_root, service) = isolated_service();
    seed_live_session(&service, SESSION);
    claim_window(&service, SESSION, TARGET + 0x10);
    force_no_foreground(&service);

    assert!(!service.focus_recovery_claim_proven_for(Some(TARGET), Some(SESSION)));
}

#[test]
fn must_fail_sessionless_or_unresolved_requests_are_not_proof() {
    let (_root, service) = isolated_service();
    seed_live_session(&service, SESSION);
    claim_window(&service, SESSION, TARGET);
    force_no_foreground(&service);

    assert!(!service.focus_recovery_claim_proven_for(Some(TARGET), None));
    assert!(!service.focus_recovery_claim_proven_for(None, Some(SESSION)));
    assert_eq!(
        gate_code(&service, false).as_deref(),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
}
