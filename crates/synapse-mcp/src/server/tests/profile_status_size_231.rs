//! Regression tests for #231: a plain `profile operation=status` read must fit
//! in an MCP client response. The full per-facade operation contract list
//! (tens of KB, already discoverable through tools/list) is summarized by its
//! sha256 and counts instead of being inlined.

use tempfile::TempDir;

use super::*;
use crate::test_support;

/// Comfortably under the ~100 KB inline limit of MCP clients, with room for
/// `profile operation=set`, which returns a before and an after snapshot.
const STATUS_RESPONSE_BUDGET_BYTES: usize = 16_000;

fn status_json(temp: &TempDir) -> anyhow::Result<serde_json::Value> {
    let service = test_support::service_in(temp.path())?;
    // A session id resolves to the default normal_agent profile, which hides
    // implementation tools and so carries hidden-tool route rows.
    let snapshot = service
        .tool_profile_snapshot(Some("profile-status-231"))
        .map_err(|error| anyhow::anyhow!("tool_profile_snapshot failed: {}", error.message))?;
    // Same envelope `profile operation=status` returns (operation, source of
    // truth, status), so every facade-added field counts against the budget.
    Ok(serde_json::to_value(ProfileResponse {
        operation: ProfileOperation::Status,
        source_of_truth: TOOL_PROFILE_SOURCE_OF_TRUTH,
        status: Some(ToolProfileStatusResponse {
            snapshot: ProfileSummary::of(&snapshot),
        }),
        set: None,
    })?)
}

#[test]
fn must_pass_default_profile_status_fits_a_client_response_and_keeps_the_contract_summary()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let status = status_json(&temp)?;
    let bytes = serde_json::to_vec(&status)?.len();
    assert!(
        bytes < STATUS_RESPONSE_BUDGET_BYTES,
        "profile status is {bytes} bytes"
    );
    assert_eq!(status["operation"], serde_json::json!("status"));
    let contract = SynapseService::facade_contract_snapshot()
        .map_err(|error| anyhow::anyhow!("facade_contract_snapshot failed: {}", error.message))?;
    assert_eq!(contract.contract_tool_count, PUBLIC_TOOL_LIMIT);
    assert!(contract.facade_contract_sha256.starts_with("sha256:"));
    Ok(())
}

#[test]
fn must_pass_shared_hidden_route_policy_is_stated_once() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let status = status_json(&temp)?;
    let policy = &status["status"]["snapshot"]["hidden_tool_route_policy"];
    for field in [
        "status",
        "agent_logical_foreground_policy",
        "human_os_foreground_policy",
        "break_glass_policy",
    ] {
        assert!(
            policy[field].as_str().is_some_and(|text| !text.is_empty()),
            "missing shared route policy field {field}"
        );
    }
    Ok(())
}

#[cfg(feature = "livefeed")]
#[tokio::test]
async fn must_fail_profile_status_after_normal_profile_set_stays_under_budget_231()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let context =
        test_support::request_context_for_session("23100000-0000-4000-8000-000000000231").await;
    service
        .profile(
            Parameters(ProfileParams {
                operation: ProfileOperation::Set,
                profile: Some(ToolProfileKind::NormalAgent),
                reason: Some("profile status budget regression #231".into()),
                confirm_break_glass: false,
            }),
            context.clone(),
        )
        .await?;
    let status = service
        .profile(
            Parameters(ProfileParams {
                operation: ProfileOperation::Status,
                profile: None,
                reason: None,
                confirm_break_glass: false,
            }),
            context,
        )
        .await?
        .0;
    let snapshot = &status
        .status
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing status"))?
        .snapshot;
    assert_eq!(snapshot.visible_tool_count, 44);
    assert_eq!(snapshot.profile, ToolProfileKind::NormalAgent);
    let bytes = serde_json::to_vec(&status)?.len();
    assert!(
        bytes < STATUS_RESPONSE_BUDGET_BYTES,
        "profile status after normal profile set is {bytes} bytes"
    );
    Ok(())
}

#[test]
fn must_fail_hidden_route_rows_do_not_repeat_the_shared_policy() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let snapshot = serde_json::to_value(
        service
            .tool_profile_snapshot(Some("profile-status-231"))
            .map_err(|error| anyhow::anyhow!("tool_profile_snapshot failed: {}", error.message))?,
    )?;
    let routes = snapshot["hidden_tool_routes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("hidden_tool_routes missing"))?;
    assert!(!routes.is_empty());
    for route in routes {
        let keys = route
            .as_object()
            .map(|row| row.keys().map(String::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        assert_eq!(keys, ["hidden_tool", "preferred_tools"], "row {route}");
    }
    Ok(())
}

#[test]
fn must_fail_full_per_facade_contract_list_is_not_inlined() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let status = status_json(&temp)?;
    assert!(
        status["status"]["snapshot"]["facade_contract"]
            .get("contracts")
            .is_none(),
        "the per-facade contract list must not be inlined in a status read"
    );
    Ok(())
}
