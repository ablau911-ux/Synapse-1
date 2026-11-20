//! #92: real act handlers, real UIA controls and readback; no foreground input.
use super::*;
use crate::server::g4_unit_support::{
    claim_window, seed_live_session, use_synthetic_notepad_foreground,
};
use crate::test_support::request_context_for_session;

#[path = "related_native_controls_92.rs"]
mod controls;

const SESSION: &str = "tf18-background92";

fn fixture() -> (tempfile::TempDir, SynapseService, controls::Controls) {
    let dir = tempfile::tempdir().expect("temporary database");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("service");
    seed_live_session(&service, SESSION);
    // Preflight stays deterministic; actual related-evidence reads use the OS shim.
    use_synthetic_notepad_foreground(&service);
    let controls = controls::Controls::spawn();
    for hwnd in [controls.checkbox, controls.edit] {
        claim_window(&service, SESSION, hwnd);
        assert_ne!(
            ::synapse_a11y::current_foreground_context()
                .ok()
                .map(|c| c.hwnd),
            Some(hwnd)
        );
    }
    (dir, service, controls)
}

async fn click(service: &SynapseService, controls: &controls::Controls) {
    let before = controls.checked();
    assert!(!before, "checkbox starts unchecked");
    let id = ::synapse_a11y::snapshot_window_from_hwnd(controls.checkbox, 1)
        .expect("checkbox tree")
        .root;
    let params: ActClickParams = serde_json::from_value(json!({
        "target": {"element_id": id}, "verify_delta": true,
        "auto_wait": false, "verify_timeout_ms": 1_000,
        "use_invoke_pattern": true, "coordinate_fallback_on_unsupported": false
    }))
    .expect("click params");
    let response = service
        .act_click(
            Parameters(params),
            request_context_for_session(SESSION).await,
        )
        .await
        .expect("optional foreground evidence must not refuse background UIA click")
        .0;
    assert!(!response.required_foreground);
    assert_eq!(response.postcondition.status, "observed_delta");
    assert!(response.postcondition.related_window_observed.is_none());
    assert!(
        controls.checked(),
        "separate native checkbox readback must show the delivered toggle"
    );
}

async fn type_text(service: &SynapseService, controls: &controls::Controls) {
    let id = ::synapse_a11y::snapshot_window_from_hwnd(controls.edit, 1)
        .expect("edit tree")
        .root;
    assert_eq!(controls.text(), "before");
    let params: ActTypeParams = serde_json::from_value(json!({
        "text": "TF18 background text", "into_element": id,
        "verify_delta": true, "auto_wait": false, "verify_timeout_ms": 1_000
    }))
    .expect("type params");
    let response = service
        .act_type(
            Parameters(params),
            request_context_for_session(SESSION).await,
        )
        .await
        .expect("background ValuePattern typing must not require OS foreground")
        .0;
    assert!(!response.required_foreground);
    assert!(response.postcondition.related_window_observed.is_none());
    assert_eq!(
        controls.text(),
        "TF18 background text",
        "separate native text readback"
    );
}

#[tokio::test]
async fn must_pass_background_handlers_when_os_foreground_is_not_the_target_92() {
    let (_dir, service, controls) = fixture();
    let foreground = ::synapse_a11y::current_foreground_context()
        .expect("non-target foreground case requires a readable OS foreground");
    assert_ne!(foreground.hwnd, controls.checkbox);
    assert_ne!(foreground.hwnd, controls.edit);
    let _probe = synapse_a11y::probe(false, controls.checkbox);
    click(&service, &controls).await;
    assert_eq!(
        synapse_a11y::target_captures(),
        0,
        "background handler must skip optional related capture"
    );
    type_text(&service, &controls).await;
}

#[tokio::test]
async fn must_pass_background_handlers_when_optional_os_foreground_is_unreadable_92() {
    let (_dir, service, controls) = fixture();
    let _unreadable = synapse_a11y::probe(true, controls.checkbox);
    click(&service, &controls).await;
    assert_eq!(
        synapse_a11y::foreground_reads(),
        1,
        "real click handler must reach the injected optional probe"
    );
    assert_eq!(
        synapse_a11y::target_captures(),
        0,
        "unreadable foreground must skip optional related capture"
    );
    type_text(&service, &controls).await;
    assert_eq!(
        synapse_a11y::foreground_reads(),
        1,
        "ValuePattern typing must skip optional foreground collection"
    );
}
