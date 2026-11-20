//! #108: the normal-profile `act verb=type` may use foreground keyboard input
//! only when the SAME session selected and owns a live claim on that exact
//! native window AND that window is the current foreground; every other case
//! is refused (`TOOL_PROFILE_POLICY_DENIED`) before any lease or input.
//!
//! Contract: #108 acceptance comment (commit ebbca313: "only when the same MCP
//! session selected and owns a live claim on that exact native window, the
//! window is currently OS foreground ... missing target refused ...
//! selected-but-unclaimed refused ... claimed window moved to background
//! refused"); server/m2_tools.rs `ensure_claimed_type_target`.
//!
//! Seams (NEEDS-SEAM, see G4 report):
//! - `SynapseService::ensure_claimed_type_target_for_session(&self, params:
//!   &ActTypeParams, session_id: Option<&str>) -> Result<(), ErrorData>` — the
//!   body of `ensure_claimed_type_target` after the RequestContext -> session
//!   id read; both production callers go through it.
//! - `crate::m1::synthetic_notepad_input` visible as `pub(crate)` (the
//!   foreground fixture the daemon's `SYNAPSE_MCP_SYNTHETIC_FIXTURE=notepad` uses).

use serde_json::json;
use synapse_core::error_codes;

use super::*;
use crate::server::g4_unit_support::{
    SYNTHETIC_NOTEPAD_HWND, claim_window, error_code, isolated_service, seed_live_session,
    set_window_target, use_synthetic_notepad_foreground,
};

const SESSION: &str = "g4-108-session";
const PEER: &str = "g4-108-peer";
const OTHER_WINDOW: i64 = 0x0010_8000;

fn type_params(hwnd: Option<i64>) -> ActTypeParams {
    let mut value = json!({"text": "oil"});
    if let Some(hwnd) = hwnd {
        value["verify_target_window_hwnd"] = json!(hwnd);
    }
    serde_json::from_value(value).expect("act_type params")
}

fn refusal(service: &SynapseService, params: &ActTypeParams, session: Option<&str>) -> String {
    let error = service
        .ensure_claimed_input_target_for_session(
            "act_type",
            params.verify_target_window_hwnd,
            session,
        )
        .expect_err("must be refused");
    assert_eq!(
        error_code(&error).as_deref(),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED),
        "{error:?}"
    );
    error.message.to_string()
}

fn fixture() -> (tempfile::TempDir, SynapseService) {
    let (root, service) = isolated_service();
    seed_live_session(&service, SESSION);
    use_synthetic_notepad_foreground(&service);
    (root, service)
}

#[test]
fn must_pass_selected_claimed_foreground_window_admits_claimed_typing() {
    let (_root, service) = fixture();
    set_window_target(&service, SESSION, SYNTHETIC_NOTEPAD_HWND);
    claim_window(&service, SESSION, SYNTHETIC_NOTEPAD_HWND);

    let result = service.ensure_claimed_input_target_for_session(
        "act_type",
        type_params(Some(SYNTHETIC_NOTEPAD_HWND)).verify_target_window_hwnd,
        Some(SESSION),
    );
    assert_eq!(result.map_err(|error| error.message.to_string()), Ok(()));
}

#[test]
fn must_fail_without_a_session_or_target_readback() {
    let (_root, service) = fixture();
    let message = refusal(&service, &type_params(Some(SYNTHETIC_NOTEPAD_HWND)), None);
    assert_eq!(message, "claimed typing requires an MCP session");
    let message = refusal(&service, &type_params(None), Some(SESSION));
    assert_eq!(message, "claimed typing requires a target window readback");
}

#[test]
fn must_fail_selected_but_unclaimed_window() {
    let (_root, service) = fixture();
    set_window_target(&service, SESSION, SYNTHETIC_NOTEPAD_HWND);
    let message = refusal(
        &service,
        &type_params(Some(SYNTHETIC_NOTEPAD_HWND)),
        Some(SESSION),
    );
    assert_eq!(
        message,
        "claimed typing requires the session's claimed window to be foreground"
    );
}

#[test]
fn must_fail_claimed_but_not_selected_window() {
    let (_root, service) = fixture();
    set_window_target(&service, SESSION, OTHER_WINDOW);
    claim_window(&service, SESSION, SYNTHETIC_NOTEPAD_HWND);
    refusal(
        &service,
        &type_params(Some(SYNTHETIC_NOTEPAD_HWND)),
        Some(SESSION),
    );
}

#[test]
fn must_fail_claimed_window_that_is_not_foreground() {
    let (_root, service) = fixture();
    set_window_target(&service, SESSION, OTHER_WINDOW);
    claim_window(&service, SESSION, OTHER_WINDOW);
    refusal(&service, &type_params(Some(OTHER_WINDOW)), Some(SESSION));
}

#[test]
fn must_fail_window_claimed_by_a_peer_session() {
    let (_root, service) = fixture();
    seed_live_session(&service, PEER);
    set_window_target(&service, SESSION, SYNTHETIC_NOTEPAD_HWND);
    claim_window(&service, PEER, SYNTHETIC_NOTEPAD_HWND);
    refusal(
        &service,
        &type_params(Some(SYNTHETIC_NOTEPAD_HWND)),
        Some(SESSION),
    );
}
