//! Proof that a target-claim owner is gone before `target_claim_adopt` tears
//! its session down (#201).
//!
//! A shared self-reported `clientInfo.name` (every Claude Code or Codex
//! instance reports the same one) and an empty in-flight ledger only show that
//! the owner is idle. An agent between tool calls is idle but alive, so neither
//! may authorize killing its session, processes and browser tabs. Only
//! independent evidence does: here, the process table for the agent process
//! tree Synapse itself launched and recorded on the session row.

use serde::Serialize;

use crate::server::session_registry::SessionRegistryRead;

/// Independent evidence that the owner's client is gone.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::server) enum OwnerGoneProof {
    /// Every process recorded for the owner's spawned agent has exited.
    SpawnedProcessesDead { probed: Vec<u32> },
}

/// `Ok` only when the owner is provably gone; otherwise `Err` with the reason
/// adoption must be refused. `process_exists` is the process-table probe.
pub(in crate::server) fn prove_owner_gone(
    owner: &SessionRegistryRead,
    process_exists: &dyn Fn(u32) -> bool,
) -> Result<OwnerGoneProof, String> {
    let Some(spawned) = owner.spawned_agent.as_ref() else {
        return Err(
            "owner session is live and not linked to a process Synapse can probe; a shared client name does not prove the owner is gone, so it will not be torn down. Wait for its claim to expire or for the owner to release it"
                .to_owned(),
        );
    };
    let mut probed = Vec::new();
    for pid in [
        spawned.agent_process_id.unwrap_or(0),
        spawned.launcher_process_id,
    ] {
        if pid != 0 && !probed.contains(&pid) {
            probed.push(pid);
        }
    }
    if probed.is_empty() {
        return Err(
            "owner spawned agent has no recorded process ids to prove it is gone".to_owned(),
        );
    }
    let running: Vec<u32> = probed
        .iter()
        .copied()
        .filter(|pid| process_exists(*pid))
        .collect();
    if !running.is_empty() {
        return Err(format!(
            "owner agent process(es) {running:?} are still running; the owner is alive"
        ));
    }
    Ok(OwnerGoneProof::SpawnedProcessesDead { probed })
}
