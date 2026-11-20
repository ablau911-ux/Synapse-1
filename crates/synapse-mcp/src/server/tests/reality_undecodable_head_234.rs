//! Regression for #234: a stored reality head row that this build cannot
//! decode (e.g. written by another build with an extra `target_binding` field)
//! must not wedge reality for its own profile nor fail scans for every caller.

use std::fs;

use tempfile::TempDir;

use super::tests::{
    install_synthetic_input, service_with_profile_dir, synthetic_input, write_profile,
};
use super::*;

async fn baseline(
    service: &SynapseService,
    profile_id: &str,
    epoch: Option<&str>,
) -> anyhow::Result<RealityBaselineResponse> {
    Ok(service
        .reality_baseline(Parameters(RealityBaselineParams {
            profile_id: Some(profile_id.to_owned()),
            epoch_id: epoch.map(ToOwned::to_owned),
            force_new_epoch: epoch.is_some(),
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
        }))
        .await?
        .0)
}

async fn synthetic_service(temp: &TempDir) -> anyhow::Result<SynapseService> {
    let profile_dir = temp.path().join("profiles");
    fs::create_dir_all(&profile_dir)?;
    write_profile(
        &profile_dir.join("synthetic.toml"),
        "synthetic",
        "single_player",
    )?;
    let service = service_with_profile_dir(temp.path(), &profile_dir)?;
    install_synthetic_input(&service, synthetic_input("Synthetic Window"))?;
    Ok(service)
}

/// Rewrites a stored head row as a foreign build would: same fields plus an
/// unknown `target_binding`, under `profile_key`, carrying `epoch_id`.
fn plant_foreign_head(
    service: &SynapseService,
    template: &RealityHeadRow,
    profile_key: &str,
    epoch_id: &str,
) -> anyhow::Result<()> {
    let mut row = serde_json::to_value(template)?;
    row["profile_key"] = serde_json::json!(profile_key);
    row["profile_id"] = serde_json::json!(profile_key);
    row["epoch_id"] = serde_json::json!(epoch_id);
    row["target_binding"] = serde_json::json!({"kind": "window", "window_hwnd": 4660});
    let runtime = service.reflex_runtime()?;
    let runtime = runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("reflex runtime lock poisoned"))?;
    runtime.storage_put_kv_rows(vec![(
        head_key(profile_key).into_bytes(),
        serde_json::to_vec(&row)?,
    )])?;
    Ok(())
}

#[tokio::test]
async fn must_pass_baseline_rebuilds_a_profile_whose_head_row_is_undecodable() -> anyhow::Result<()>
{
    let temp = TempDir::new()?;
    let service = synthetic_service(&temp).await?;
    let first = baseline(&service, "synthetic", Some("original-epoch")).await?;
    plant_foreign_head(&service, &first.head, "synthetic", "original-epoch")?;

    let rebuilt = baseline(&service, "synthetic", None).await?;

    assert!(rebuilt.created, "an undecodable head must be rebuilt");
    assert_eq!(rebuilt.profile_key, "synthetic");
    assert_eq!(
        rebuilt.reason.as_deref(),
        Some("head_row_undecodable_rebuilt"),
        "the rebuild must say why"
    );
    let stored = service.lookup_reality_head("synthetic")?.into_head();
    assert_eq!(
        stored.map(|head| head.epoch_id),
        Some(rebuilt.head.epoch_id)
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_delta_on_a_profile_with_an_undecodable_head_reports_why() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = synthetic_service(&temp).await?;
    let first = baseline(&service, "synthetic", Some("original-epoch")).await?;
    plant_foreign_head(&service, &first.head, "synthetic", "original-epoch")?;

    let response = service
        .observe_delta(Parameters(ObserveDeltaParams {
            profile_id: Some("synthetic".to_owned()),
            since_epoch: Some("original-epoch".to_owned()),
            since_seq: Some(0),
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
            max_deltas: DEFAULT_MAX_DELTAS,
        }))
        .await?
        .0;

    assert!(response.baseline_required);
    assert!(response.rebase_required);
    assert_eq!(response.profile_key.as_deref(), Some("synthetic"));
    assert_eq!(
        response.reason.as_deref(),
        Some(
            "missing_baseline: stored head row is undecodable; call reality baseline to rebuild it"
        ),
        "reason must name the undecodable head"
    );
    assert!(response.deltas.is_empty());
    Ok(())
}

#[tokio::test]
async fn must_fail_audit_on_a_profile_with_an_undecodable_head_reports_why() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = synthetic_service(&temp).await?;
    let first = baseline(&service, "synthetic", Some("original-epoch")).await?;
    plant_foreign_head(&service, &first.head, "synthetic", "original-epoch")?;

    let response = service
        .reality_audit(Parameters(RealityAuditParams {
            profile_id: Some("synthetic".to_owned()),
            epoch_id: None,
            assumption_hash: None,
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
        }))
        .await?
        .0;

    assert!(response.rebase_required);
    assert!(response.audit.rebase_required);
    assert_eq!(
        response.audit.rebase_reason.as_deref(),
        Some("stored head row is undecodable; call reality baseline to rebuild it"),
        "audit must name the undecodable head"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_undecodable_head_of_another_profile_is_never_matched_to_an_epoch()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = synthetic_service(&temp).await?;
    let first = baseline(&service, "synthetic", Some("synthetic-epoch")).await?;
    plant_foreign_head(&service, &first.head, "foreign", "foreign-epoch")?;

    let response = service
        .observe_delta(Parameters(ObserveDeltaParams {
            profile_id: None,
            since_epoch: Some("foreign-epoch".to_owned()),
            since_seq: Some(0),
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
            max_deltas: DEFAULT_MAX_DELTAS,
        }))
        .await?
        .0;

    // The poll succeeds and rebases against synthetic's own head: the
    // undecodable foreign row is never matched as a profile switch.
    assert!(response.rebase_required);
    assert!(!response.baseline_required);
    assert_eq!(response.profile_key.as_deref(), Some("synthetic"));
    assert_eq!(
        response.reason.as_deref(),
        Some("stale_epoch: requested foreign-epoch, current synthetic-epoch")
    );
    Ok(())
}
