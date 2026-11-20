//! Regression tests for #112 driving `SynapseService::open_session_ids` and
//! `m1_state.capture_owner` directly on `SynapseService` (no `#[path]` copy):
//! verifies interleaved session registration after `open_session_ids()`,
//! live owner protection, and release after `SessionRegistry::record_closed`.

use anyhow::{Context, ensure};

use super::SynapseService;

#[test]
fn must_fail_interleaved_registration_after_open_session_ids_snapshot_protects_new_owner()
-> anyhow::Result<()> {
    let dir = tempfile::TempDir::new().context("tempdir")?;
    let service = SynapseService::try_new_isolated_for_tests(dir.path())?;
    service
        .session_registry_ref()
        .lock()
        .map_err(|_| anyhow::anyhow!("session_registry lock"))?
        .record_seen("session-a", None, 1_000);

    // Caller A snapshots open sessions BEFORE session-b connects and claims capture.
    let stale_snapshot = service
        .open_session_ids()
        .map_err(|error| anyhow::anyhow!("open_session_ids: {}", error.message))?;
    ensure!(
        !stale_snapshot.ids.contains("session-b"),
        "precondition: session-b is absent from stale snapshot"
    );

    service
        .session_registry_ref()
        .lock()
        .map_err(|_| anyhow::anyhow!("session_registry lock"))?
        .record_seen("session-b", None, 1_001);
    let mut state = service
        .m1_state()
        .map_err(|error| anyhow::anyhow!("m1_state: {}", error.message))?;
    state.capture_owner.claim("cap-112", Some("session-b"));

    let conflict = state
        .capture_owner
        .check_mutation_with_snapshot(
            Some("cap-112"),
            Some("session-a"),
            stale_snapshot.taken_at,
            |session| stale_snapshot.ids.contains(session),
        )
        .err()
        .context(
            "stale snapshot must still refuse session-a when session-b claimed after taken_at",
        )?;
    ensure!(
        conflict.owner_session_id == "session-b" && conflict.capture_id == "cap-112",
        "unexpected conflict readback: {conflict:?}"
    );
    Ok(())
}

#[test]
fn must_pass_closed_session_in_registry_releases_capture_on_next_snapshot() -> anyhow::Result<()> {
    let dir = tempfile::TempDir::new().context("tempdir")?;
    let service = SynapseService::try_new_isolated_for_tests(dir.path())?;
    {
        let mut registry = service
            .session_registry_ref()
            .lock()
            .map_err(|_| anyhow::anyhow!("session_registry lock"))?;
        registry.record_seen("session-a", None, 1_000);
        registry.record_seen("session-b", None, 1_001);
    }
    {
        let mut state = service
            .m1_state()
            .map_err(|error| anyhow::anyhow!("m1_state: {}", error.message))?;
        state.capture_owner.claim("cap-112", Some("session-b"));
    }
    service
        .session_registry_ref()
        .lock()
        .map_err(|_| anyhow::anyhow!("session_registry lock"))?
        .record_closed("session-b", 2_000);

    let after_close = service
        .open_session_ids()
        .map_err(|error| anyhow::anyhow!("open_session_ids: {}", error.message))?;
    ensure!(
        !after_close.ids.contains("session-b") && after_close.ids.contains("session-a"),
        "closed session-b must be excluded from open_session_ids"
    );
    let state = service
        .m1_state()
        .map_err(|error| anyhow::anyhow!("m1_state: {}", error.message))?;
    state
        .capture_owner
        .check_mutation_with_snapshot(
            Some("cap-112"),
            Some("session-a"),
            after_close.taken_at,
            |session| after_close.ids.contains(session),
        )
        .map_err(|conflict| anyhow::anyhow!("closed owner must release capture: {conflict:?}"))?;
    Ok(())
}

fn live_owner_service() -> anyhow::Result<(tempfile::TempDir, SynapseService)> {
    let dir = tempfile::TempDir::new().context("tempdir")?;
    let service = SynapseService::try_new_isolated_for_tests(dir.path())?;
    {
        let mut registry = service
            .session_registry_ref()
            .lock()
            .map_err(|_| anyhow::anyhow!("session_registry lock"))?;
        let now = crate::server::session_registry::unix_time_ms_now();
        registry.record_seen("owner-112", None, now);
        registry.record_seen("intruder-112", None, now);
    }
    let mut state = service
        .m1_state()
        .map_err(|error| anyhow::anyhow!("m1_state: {}", error.message))?;
    state.capture_owner.claim("cap-live", Some("owner-112"));
    drop(state);
    Ok((dir, service))
}

#[test]
fn must_fail_live_registered_owner_blocks_non_owner_but_allows_unscoped_admin() -> anyhow::Result<()>
{
    let (_dir, service) = live_owner_service()?;
    let open = service
        .open_session_ids()
        .map_err(|error| anyhow::anyhow!("open_session_ids: {}", error.message))?;
    let state = service
        .m1_state()
        .map_err(|error| anyhow::anyhow!("m1_state: {}", error.message))?;

    // #112 protects other sessions; the unscoped stdio admin lane is allowed.
    for caller in [Some("intruder-112"), None, Some("owner-112")] {
        let result = state.capture_owner.check_mutation_with_snapshot(
            Some("cap-live"),
            caller,
            open.taken_at,
            |session| open.ids.contains(session),
        );
        if caller != Some("intruder-112") {
            result
                .map_err(|conflict| anyhow::anyhow!("authorized caller refused: {conflict:?}"))?;
            continue;
        }
        let conflict = result.err().context("non-owner caller must be refused")?;
        ensure!(
            conflict.message("stop").contains("owner-112"),
            "refusal message must name owner-112: {}",
            conflict.message("stop")
        );
    }
    Ok(())
}
