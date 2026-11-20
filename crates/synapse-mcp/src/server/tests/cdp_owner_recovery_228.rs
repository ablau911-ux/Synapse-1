//! #228: CDP owner recovery must follow the #201 rule. A shared self-reported
//! client name does not prove the persisted owner is gone, so a live peer
//! session keeps its CDP target; recovery is allowed only from a terminated
//! owner or one whose recorded agent processes are all dead. Since #298 a live
//! owner's lock is kept by the idle tab-lock policy, which refuses first.
use synapse_core::error_codes;
use tempfile::TempDir;

use super::{
    CdpTargetOwner, SynapseService,
    tests::{
        insert_test_target_claim, seed_session_client, seed_spawned_child, service_with_temp_db,
    },
};

const CLIENT: &str = "claude-code";

/// Seeds a live owner session that persisted a CDP target owner row, then a
/// newer live requester on the same client name holding the target claim.
fn seed_owner_and_requester(
    service: &SynapseService,
    owner_session: &str,
    requester_session: &str,
    target_id: &str,
    hwnd: i64,
) -> anyhow::Result<()> {
    let now = crate::server::session_registry::unix_time_ms_now();
    seed_session_client(service, owner_session, CLIENT, now.saturating_sub(1_000))?;
    seed_session_client(service, requester_session, CLIENT, now)?;
    service.register_cdp_target_owner(CdpTargetOwner {
        session_id: owner_session.to_owned(),
        window_hwnd: hwnd,
        endpoint: "chrome-extension://test/chrome.tabs".to_owned(),
        chrome_window_id: None,
        capture_window_hwnd: None,
        cdp_target_id: target_id.to_owned(),
        requested_url: "about:blank#228".to_owned(),
        target_url: "about:blank#228".to_owned(),
        created_at_unix_ms: now,
    })?;
    service.cdp_target_owners_ref().lock().unwrap().clear();
    insert_test_target_claim(service, requester_session, hwnd, target_id)
}

/// #298 (77e25dfe): the idle tab-lock policy decides before the #201
/// adoption rule. A live owner keeps its lock. Unit tests configure no daemon
/// lifecycle ledger, so the owner's activity reads as unknown, and an unknown
/// owner keeps its lock too (it is never treated as idle).
fn assert_live_owner_keeps_tab_lock(
    refusal: &rmcp::ErrorData,
    owner: &str,
    requester: &str,
    target_id: &str,
) {
    use crate::server::target_claims::tab_lock_idle::{
        KeepReason, idle_release_ms, tab_lock_refusal_message,
    };
    assert_eq!(
        refusal.data.as_ref().and_then(|data| data.get("code")),
        Some(&serde_json::json!(error_codes::ACTION_TARGET_INVALID))
    );
    assert_eq!(
        refusal.message,
        tab_lock_refusal_message(
            "cdp_close_tab",
            target_id,
            owner,
            requester,
            KeepReason::UnknownActivity,
            idle_release_ms(),
        ),
        "unexpected refusal"
    );
}

fn persisted_owner_session(service: &SynapseService, target_id: &str) -> anyhow::Result<String> {
    let rows = service.read_persisted_cdp_target_owners_for_target_id(target_id)?;
    assert_eq!(rows.len(), 1);
    Ok(rows[0].1.owner_session_id.clone())
}

#[test]
fn must_fail_live_peer_with_shared_client_name_keeps_its_cdp_owner_row() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (owner, requester) = ("i228-live-owner", "i228-live-requester");
    let target_id = "chrome-tab:i228-live-peer";
    seed_owner_and_requester(&service, owner, requester, target_id, 0x2281)?;

    let refusal = service
        .cdp_target_owner_for_close(requester, target_id, None)
        .expect_err("a live owner with no probeable process must not lose its CDP target");

    assert_live_owner_keeps_tab_lock(&refusal, owner, requester, target_id);
    assert_eq!(persisted_owner_session(&service, target_id)?, owner);
    Ok(())
}

#[test]
fn must_pass_live_owner_whose_recorded_processes_are_all_dead_is_recovered_from()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (owner, requester) = ("i228-dead-owner", "i228-dead-requester");
    let target_id = "chrome-tab:i228-dead-agent";
    seed_owner_and_requester(&service, owner, requester, target_id, 0x2283)?;
    let now = crate::server::session_registry::unix_time_ms_now();
    seed_spawned_child(
        &service,
        owner,
        "i228-unrelated-parent",
        "agent-spawn-i228-dead",
        u32::MAX,
        Some(u32::MAX - 1),
        dir.path(),
        now.saturating_sub(900),
    )?;

    let (_key, recovered) = service.cdp_target_owner_for_close(requester, target_id, None)?;

    assert_eq!(recovered.session_id, requester);
    assert_eq!(persisted_owner_session(&service, target_id)?, requester);
    Ok(())
}

#[test]
fn must_fail_live_owner_with_a_rehydrated_session_id_keeps_its_cdp_owner_row() -> anyhow::Result<()>
{
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (owner, requester) = ("i228-rehydrated-owner", "i228-rehydrated-requester");
    let target_id = "chrome-tab:i228-rehydrated";
    seed_owner_and_requester(&service, owner, requester, target_id, 0x2285)?;
    // The owner's session id is live again after a restart: it now started
    // after the owner row was stored.
    let later = crate::server::session_registry::unix_time_ms_now().saturating_add(10_000);
    service
        .session_registry_ref()
        .lock()
        .map_err(|_err| anyhow::anyhow!("session registry lock poisoned"))?
        .set_started_at_for_test(owner, later);

    let refusal = service
        .cdp_target_owner_for_close(requester, target_id, None)
        .expect_err("a rehydrated live owner must not lose its CDP target by client name");

    assert_live_owner_keeps_tab_lock(&refusal, owner, requester, target_id);
    assert_eq!(persisted_owner_session(&service, target_id)?, owner);
    Ok(())
}

#[test]
fn must_pass_terminated_owner_is_recovered_from_by_the_same_client() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (owner, requester) = ("i228-closed-owner", "i228-closed-requester");
    let target_id = "chrome-tab:i228-closed";
    seed_owner_and_requester(&service, owner, requester, target_id, 0x2287)?;
    let now = crate::server::session_registry::unix_time_ms_now();
    assert!(
        service
            .session_registry_ref()
            .lock()
            .map_err(|_err| anyhow::anyhow!("session registry lock poisoned"))?
            .record_closed(owner, now)
    );

    let (_key, recovered) = service.cdp_target_owner_for_close(requester, target_id, None)?;

    assert_eq!(recovered.session_id, requester);
    assert_eq!(persisted_owner_session(&service, target_id)?, requester);
    Ok(())
}
