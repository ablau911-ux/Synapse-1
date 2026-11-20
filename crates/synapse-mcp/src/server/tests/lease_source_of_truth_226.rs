//! Regression for #226: the dashboard lease force-release/handoff
//! source_of_truth must name CF_SESSIONS, where the session lease rows live.

use tempfile::TempDir;

use super::*;
use crate::test_support;

/// Drives the real dashboard force-release on a persisted session lease row:
/// the response names CF_SESSIONS exactly, and the CF_SESSIONS row it names is
/// the one that is gone afterwards.
#[tokio::test]
async fn must_pass_dashboard_lease_source_of_truth_names_cf_sessions() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("lease_source_of_truth_226");
    let temp = TempDir::new()?;
    let service = test_support::service_in(temp.path())?;
    let db = service
        .m3_state_handle()
        .lock()
        .map_err(|_error| anyhow::anyhow!("m3 state poisoned"))?
        .ensure_storage()?;
    let owner = "i226-owner";
    assert!(matches!(
        lease::try_acquire(owner, lease::ttl_from_ms(5_000)),
        LeaseOutcome::Acquired(_)
    ));
    service
        .persist_session_lease(owner, &lease::status())
        .map_err(|error| anyhow::anyhow!("persist lease row: {error:?}"))?;
    let sessions_before = db.scan_cf(synapse_storage::cf::CF_SESSIONS)?.len();
    assert!(sessions_before >= 1, "the session lease row must exist");

    let response = service
        .dashboard_control_lease_force_release(owner.to_owned(), true)
        .await
        .map_err(|error| anyhow::anyhow!("force release must succeed: {error:?}"))?;

    assert_eq!(
        response.source_of_truth,
        "synapse_action::lease + CF_SESSIONS MCP session lease rows + CF_AGENT_EVENTS + CF_ACTION_LOG"
    );
    assert!(response.released, "the owner's lease must be released");
    assert!(response.persisted_row_deleted);
    assert!(!response.after.held);
    assert!(!lease::status().held);
    assert_eq!(
        db.scan_cf(synapse_storage::cf::CF_SESSIONS)?.len(),
        sessions_before - 1,
        "the CF_SESSIONS lease row must be gone"
    );
    Ok(())
}
