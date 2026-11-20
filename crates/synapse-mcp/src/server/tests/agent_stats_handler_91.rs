//! Handler-level regression tests for #91: the real `agent_stats` handler
//! reports the authoritative live lease next to the journal-derived
//! `held_open`, and the two disagree after a lease is released without a
//! journaled release event (phantom held-open).

use std::num::NonZeroUsize;

use synapse_action::lease;
use synapse_core::{AgentEventKind, AgentEventRecord};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::super::agent_events::record_agent_events_unobserved;
use super::*;
use crate::{
    m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig, server::SynapseService,
    test_support,
};

const BASE_NS: u64 = 1_000_000_000_000;

fn service(dir: &std::path::Path) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(dir.join("db")),
            Some(dir.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).ok_or_else(|| anyhow::anyhow!("nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

fn write_lease_acquired(service: &SynapseService, spawn: &str) -> anyhow::Result<()> {
    let db = service
        .agent_stats_db()
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let mut record = AgentEventRecord::new(BASE_NS, AgentEventKind::LeaseAcquired);
    record.spawn_id = Some(spawn.to_owned());
    record_agent_events_unobserved(&db, &[record])?;
    db.flush()?;
    Ok(())
}

/// Drives the real `agent_stats` tool handler (not the helper types).
fn call_handler(service: &SynapseService) -> anyhow::Result<AgentStatsResponse> {
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let params = AgentStatsParams {
        since_ns: None,
        until_ns: None,
        spawn_id: None,
        session_id: None,
        group_by: None,
    };
    let response = runtime
        .block_on(service.agent_stats(Parameters(params)))
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok(response.0)
}

#[test]
fn must_pass_handler_reports_unheld_live_lease_when_nothing_holds_it() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("agent_stats_handler_91");
    let temp = TempDir::new()?;
    let service = service(temp.path())?;

    let response = call_handler(&service)?;

    assert!(response.ok);
    assert!(!response.live_lease.held);
    assert_eq!(response.live_lease.owner_session_id, None);
    assert_eq!(response.live_lease.expires_in_ms, None);
    Ok(())
}

#[test]
fn must_pass_handler_reports_held_live_lease_owner_from_physical_state() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("agent_stats_handler_91");
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    let outcome = lease::try_acquire("session-a", lease::ttl_from_ms(30_000));
    assert!(matches!(outcome, lease::LeaseOutcome::Acquired(_)));

    let response = call_handler(&service)?;

    assert!(response.live_lease.held);
    assert_eq!(
        response.live_lease.owner_session_id.as_deref(),
        Some("session-a")
    );
    assert!(response.live_lease.expires_in_ms.is_some());
    Ok(())
}

#[test]
fn must_fail_handler_phantom_held_open_disagrees_with_live_lease_after_unjournaled_release()
-> anyhow::Result<()> {
    let _serial = test_support::lease_serial("agent_stats_handler_91");
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    write_lease_acquired(&service, "agent-spawn-91")?;
    let outcome = lease::try_acquire("session-a", lease::ttl_from_ms(30_000));
    assert!(matches!(outcome, lease::LeaseOutcome::Acquired(_)));

    let while_held = call_handler(&service)?;
    assert_eq!(while_held.fleet.leases.held_open, 1);
    assert!(while_held.live_lease.held);

    // Physical release with no journal row: the journal balance stays at 1.
    lease::release("session-a").map_err(|error| anyhow::anyhow!("{error:?}"))?;

    let after_release = call_handler(&service)?;
    assert_eq!(after_release.fleet.leases.held_open, 1);
    assert!(!after_release.live_lease.held);
    assert_eq!(after_release.live_lease.owner_session_id, None);
    Ok(())
}
