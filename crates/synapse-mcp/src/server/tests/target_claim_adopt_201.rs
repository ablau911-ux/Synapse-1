//! #201: `target_claim_adopt` tears the owner session down, so a shared
//! self-reported client name and idleness are not enough: the owner must be
//! provably gone, and the claim must not have moved since it was read.
use std::collections::BTreeSet;

use super::{
    SessionTarget, TargetClaimRegistry,
    adopt_proof::{OwnerGoneProof, prove_owner_gone},
};
use crate::server::session_registry::{SessionRegistryRead, SpawnedAgentRead};

fn spawned(agent_pid: Option<u32>, launcher_pid: u32) -> SpawnedAgentRead {
    SpawnedAgentRead {
        spawn_id: "spawn-1".to_owned(),
        cli: "claude".to_owned(),
        launcher_process_id: launcher_pid,
        agent_process_id: agent_pid,
        started_by_session_id: None,
        launched_at_unix_ms: 1,
        launch_target: "test".to_owned(),
        log_dir: "test".to_owned(),
        template_id: None,
        template_version: None,
        control: None,
    }
}

fn owner(spawned_agent: Option<SpawnedAgentRead>) -> SessionRegistryRead {
    SessionRegistryRead {
        session_id: "owner".to_owned(),
        transport: "http".to_owned(),
        client_name: Some("claude-code".to_owned()),
        client_version: None,
        protocol_version: None,
        agent_kind: "claude".to_owned(),
        lifecycle: "live".to_owned(),
        started_at_unix_ms: 1,
        last_seen_unix_ms: 1,
        last_seen_ms_ago: 0,
        stale_after_ms: 300_000,
        closed_at_unix_ms: None,
        last_action: None,
        last_reason_code: None,
        spawned_agent,
    }
}

#[test]
fn must_fail_live_unlinked_owner_is_not_provably_gone_even_with_a_shared_client_name() {
    let refusal = prove_owner_gone(&owner(None), &|_pid| false)
        .expect_err("a live session with no probeable process must not be torn down");

    assert!(
        refusal.contains("not linked"),
        "unexpected refusal: {refusal}"
    );
}

#[test]
fn must_fail_owner_with_a_running_agent_process_is_refused() {
    let refusal = prove_owner_gone(&owner(Some(spawned(Some(4242), 100))), &|pid| pid == 4242)
        .expect_err("running agent process proves the owner is alive");

    assert!(refusal.contains("4242"), "unexpected refusal: {refusal}");
}

#[test]
fn must_fail_owner_with_a_running_launcher_process_is_refused() {
    let refusal = prove_owner_gone(&owner(Some(spawned(None, 100))), &|pid| pid == 100)
        .expect_err("running launcher keeps the owner alive");

    assert!(refusal.contains("100"), "unexpected refusal: {refusal}");
}

#[test]
fn must_pass_owner_whose_recorded_processes_are_all_dead_is_provably_gone() {
    let proof = prove_owner_gone(&owner(Some(spawned(Some(4242), 100))), &|_pid| false)
        .expect("all recorded processes are dead");

    assert_eq!(
        proof,
        OwnerGoneProof::SpawnedProcessesDead {
            probed: vec![4242, 100]
        }
    );
}

#[test]
fn must_fail_spawned_owner_without_any_recorded_pid_is_refused() {
    let refusal = prove_owner_gone(&owner(Some(spawned(None, 0))), &|_pid| false)
        .expect_err("no pid means no proof");

    assert!(
        refusal.contains("no recorded process"),
        "unexpected refusal: {refusal}"
    );
}

#[test]
fn must_fail_claim_unmoved_detects_release_and_reclaim_by_another_session() {
    let mut registry = TargetClaimRegistry::default();
    let target = SessionTarget::Window { hwnd: 0x5150 };
    let both = BTreeSet::from(["owner".to_owned(), "other".to_owned()]);
    let prior = registry
        .claim("owner", target.clone(), 10_000, 1_000, &both)
        .expect("owner claims")
        .0;
    assert!(registry.claim_unmoved(&prior), "untouched claim is unmoved");

    registry
        .release("owner", "window:0x5150")
        .expect("owner releases");
    registry
        .claim("other", target, 10_000, 1_100, &both)
        .expect("other claims");

    assert!(
        !registry.claim_unmoved(&prior),
        "claim now belongs to another session: adopt must abort before teardown"
    );
}
