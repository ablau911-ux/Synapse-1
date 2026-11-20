//! FU1 #135: restore observe-driven profile deactivation and event publication.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::server::{ObserveParams, Parameters, SynapseService};
use serde_json::json;
use synapse_core::EventFilter;

fn service(root: &std::path::Path) -> anyhow::Result<SynapseService> {
    let profile = r#"id = "notepad"
label = "notepad"
schema_version = 2
use_scope = "productivity"
mouse_velocity_profile_default = "natural"
keyboard_dynamics_default = "natural"
[[matches]]
exe = "notepad.exe"
[detection]
classes_of_interest = ["window"]
confidence_threshold = 0.50
max_detections = 8
"#;
    std::fs::write(root.join("notepad.toml"), profile)?;
    crate::test_support::service_in(root)
}

fn install(service: &SynapseService, matched: bool) {
    let mut input = crate::m1::synthetic_notepad_input();
    if !matched {
        input.foreground.process_name = "unprofiled.exe".to_owned();
        input.foreground.process_path = "C:\\Synthetic\\unprofiled.exe".to_owned();
        input.foreground.window_title = "Unprofiled App".to_owned();
        input.foreground.profile_id = None;
    }
    service.m1_state.lock().unwrap().synthetic = Some(input);
}

async fn observe(service: &SynapseService) -> anyhow::Result<synapse_core::Observation> {
    let context = crate::test_support::request_context_for_session("fu1-scope135").await;
    // No window_hwnd: an explicit HWND takes the live-window path
    // (m1::observe_input), which bypasses the installed synthetic foreground.
    let params: ObserveParams = serde_json::from_value(json!({"include":["focused"]}))?;
    Ok(tokio::time::timeout(
        std::time::Duration::from_millis(200),
        service.observe(Parameters(params), context),
    )
    .await??
    .0)
}

#[tokio::test]
async fn must_pass_observe_deactivates_profile_and_publishes_exact_scope_135() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let service = service(root.path())?;
    let subscription = service.sse_state()?.event_bus().subscribe(
        EventFilter::All,
        vec![
            "profile-changed".to_owned(),
            "scope-transitioned".to_owned(),
        ],
        false,
    )?;
    install(&service, true);
    assert_eq!(
        observe(&service).await?.foreground.profile_id.as_deref(),
        Some("notepad")
    );
    assert_eq!(
        service.profile_runtime()?.active_profile_id()?.as_deref(),
        Some("notepad")
    );
    let _initial = subscription.drain();
    install(&service, false);
    assert_eq!(observe(&service).await?.foreground.profile_id, None);
    assert_eq!(service.profile_runtime()?.active_profile_id()?, None);
    let events = subscription.drain();
    let scopes: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "scope-transitioned")
        .collect();
    assert_eq!(scopes.len(), 1, "{events:?}");
    assert_eq!(scopes[0].data["old_scope"], "productivity");
    assert_eq!(scopes[0].data["new_scope"], "unknown");
    assert_eq!(scopes[0].data["old_profile_id"], "notepad");
    assert_eq!(scopes[0].data["new_profile_id"], serde_json::Value::Null);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "profile-changed")
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_unchanged_scope_does_not_publish_duplicate_transition_135() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let service = service(root.path())?;
    let subscription = service.sse_state()?.event_bus().subscribe(
        EventFilter::All,
        vec![
            "profile-changed".to_owned(),
            "scope-transitioned".to_owned(),
        ],
        false,
    )?;
    install(&service, true);
    assert_eq!(
        observe(&service).await?.foreground.profile_id.as_deref(),
        Some("notepad")
    );
    let _initial = subscription.drain();
    assert_eq!(
        observe(&service).await?.foreground.profile_id.as_deref(),
        Some("notepad")
    );
    assert_eq!(
        service.profile_runtime()?.active_profile_id()?.as_deref(),
        Some("notepad")
    );
    assert_eq!(subscription.drain().len(), 0);
    Ok(())
}
