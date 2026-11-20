//! Regression for #156: a `reality delta` poll whose watch epoch belongs to a
//! different profile than the one now resolved must report `profile_changed`,
//! not `missing_baseline`.

use std::fs;

use tempfile::TempDir;

use super::tests::{
    install_synthetic_input, service_with_profile_dir, synthetic_input, write_profile,
};
use super::*;

async fn service_watching_synthetic(temp: &TempDir, epoch: &str) -> anyhow::Result<SynapseService> {
    let profile_dir = temp.path().join("profiles");
    fs::create_dir_all(&profile_dir)?;
    write_profile(
        &profile_dir.join("synthetic.toml"),
        "synthetic",
        "single_player",
    )?;
    write_profile(&profile_dir.join("other.toml"), "other", "single_player")?;
    let service = service_with_profile_dir(temp.path(), &profile_dir)?;
    install_synthetic_input(&service, synthetic_input("Synthetic Window"))?;
    service
        .reality_baseline(Parameters(RealityBaselineParams {
            profile_id: Some("synthetic".to_owned()),
            epoch_id: Some(epoch.to_owned()),
            force_new_epoch: true,
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
        }))
        .await?;
    Ok(service)
}

fn switch_foreground_to_other(service: &SynapseService) -> anyhow::Result<()> {
    let mut other = synthetic_input("Other Window");
    other.foreground.process_name = "other.exe".to_owned();
    other.foreground.process_path = "C:\\Synthetic\\other.exe".to_owned();
    install_synthetic_input(service, other)
}

async fn delta(
    service: &SynapseService,
    profile_id: Option<&str>,
    since_epoch: &str,
) -> anyhow::Result<ObserveDeltaResponse> {
    Ok(service
        .observe_delta(Parameters(ObserveDeltaParams {
            profile_id: profile_id.map(ToOwned::to_owned),
            since_epoch: Some(since_epoch.to_owned()),
            since_seq: Some(0),
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
            max_deltas: DEFAULT_MAX_DELTAS,
        }))
        .await?
        .0)
}

fn assert_profile_changed(response: &ObserveDeltaResponse) {
    assert!(response.rebase_required);
    assert!(!response.baseline_required);
    assert_eq!(response.profile_key.as_deref(), Some("synthetic"));
    assert_eq!(
        response.reason.as_deref(),
        Some("profile_changed: head profile synthetic but observed other")
    );
    assert!(response.deltas.is_empty());
}

#[tokio::test]
async fn must_fail_delta_without_profile_id_after_switch_reports_profile_changed()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_watching_synthetic(&temp, "switch-omitted-epoch").await?;
    switch_foreground_to_other(&service)?;
    let response = delta(&service, None, "switch-omitted-epoch").await?;
    assert_profile_changed(&response);
    Ok(())
}

#[tokio::test]
async fn must_fail_delta_with_new_profile_id_after_switch_reports_profile_changed()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_watching_synthetic(&temp, "switch-explicit-epoch").await?;
    switch_foreground_to_other(&service)?;
    let response = delta(&service, Some("other"), "switch-explicit-epoch").await?;
    assert_profile_changed(&response);
    Ok(())
}

#[tokio::test]
async fn must_pass_delta_for_unknown_epoch_on_new_profile_still_reports_missing_baseline()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_watching_synthetic(&temp, "known-epoch").await?;
    switch_foreground_to_other(&service)?;
    let response = delta(&service, None, "never-created-epoch").await?;
    assert!(response.baseline_required);
    assert!(response.rebase_required);
    assert_eq!(response.reason.as_deref(), Some("missing_baseline"));
    Ok(())
}
