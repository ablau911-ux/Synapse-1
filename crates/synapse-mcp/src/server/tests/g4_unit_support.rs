//! Shared fixtures for the G4 (act foreground gate / recovery) unit tests.
//!
//! Builds an isolated `SynapseService` (own temp DB, never the live daemon's)
//! and seeds the real in-memory registries the gate code reads: live MCP
//! sessions, target claims and session targets. Foreground state is injected
//! through the same M1 fixture flags the daemon's env fixtures set
//! (`force_no_foreground`, `synthetic`), so `current_audit_foreground` returns
//! exactly what a null `GetForegroundWindow` / synthetic Notepad returns.

use rmcp::ErrorData;
use serde_json::Value;

use crate::server::session_registry::unix_time_ms_now;
use crate::server::{SessionTarget, SynapseService};

/// HWND of the synthetic Notepad foreground (m1/sources.rs `synthetic_notepad_input`).
pub(crate) const SYNTHETIC_NOTEPAD_HWND: i64 = 0x1234;

pub(crate) fn isolated_service() -> (tempfile::TempDir, SynapseService) {
    let root = tempfile::tempdir().expect("temp dir");
    let service =
        SynapseService::try_new_isolated_for_tests(root.path()).expect("isolated service");
    (root, service)
}

/// Registers `session_id` as a live initialized MCP session (claim pruning
/// keeps only claims of live sessions).
pub(crate) fn seed_live_session(service: &SynapseService, session_id: &str) {
    use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
    let state = rmcp::transport::streamable_http_server::session::SessionState::new(
        InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("g4-unit-test", "0.0.0-test"),
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

/// Seeds a live claim of `session_id` on window `hwnd` and reads it back.
pub(crate) fn claim_window(service: &SynapseService, session_id: &str, hwnd: i64) {
    let now = unix_time_ms_now();
    let live = std::collections::BTreeSet::from([session_id.to_owned()]);
    let claimed = service.lock_target_claims().expect("claims lock").claim(
        session_id,
        SessionTarget::Window { hwnd },
        120_000,
        now,
        &live,
    );
    if let Err(conflict) = claimed {
        panic!("precondition: claim of 0x{hwnd:x} by {session_id} refused: {conflict:?}");
    }
    let read = service
        .target_claim_for_session(session_id, &SessionTarget::Window { hwnd })
        .expect("claim readback");
    assert_eq!(
        read.map(|claim| claim.owner_session_id),
        Some(session_id.to_owned()),
        "precondition: claim readback"
    );
}

/// Binds the session's active target to window `hwnd` and reads it back.
pub(crate) fn set_window_target(service: &SynapseService, session_id: &str, hwnd: i64) {
    service
        .session_targets_ref()
        .lock()
        .expect("session targets")
        .insert(session_id.to_owned(), SessionTarget::Window { hwnd });
    assert_eq!(
        service
            .session_target(Some(session_id))
            .expect("target readback"),
        Some(SessionTarget::Window { hwnd }),
        "precondition: session target"
    );
}

/// The action gate's foreground read now returns `A11Y_NO_FOREGROUND`.
pub(crate) fn force_no_foreground(service: &SynapseService) {
    service.m1_state().expect("m1 state").force_no_foreground = true;
    let read = service.current_audit_foreground();
    assert_eq!(
        read.as_ref().err().and_then(error_code),
        Some(synapse_core::error_codes::A11Y_NO_FOREGROUND.to_owned()),
        "precondition: forced no-foreground"
    );
}

/// The action gate's foreground read now returns the synthetic Notepad window.
pub(crate) fn use_synthetic_notepad_foreground(service: &SynapseService) {
    service.m1_state().expect("m1 state").synthetic = Some(crate::m1::synthetic_notepad_input());
    assert_eq!(
        service.current_audit_foreground().expect("foreground").hwnd,
        SYNTHETIC_NOTEPAD_HWND,
        "precondition: synthetic foreground"
    );
}

pub(crate) fn error_code(error: &ErrorData) -> Option<String> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}
