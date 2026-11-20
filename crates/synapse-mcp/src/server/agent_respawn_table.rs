//! Frozen behaviour table for #308 row 308a: when respawn may launch a new copy.
//!
//! Written by the manager from the behaviour table in issue #308 (work method v3).
//! Code agents must not edit this file. Deviation logged in #308: written after the
//! code; red evidence is the old build's `.unwrap_or(false)` at `agent_control.rs`
//! (4b003b82), which launched a new copy on every row below.

use super::{PriorProcess, respawn_prior_gone};

/// (row name, prior process state, expected: respawn proceeds)
fn rows() -> Vec<(&'static str, PriorProcess, bool)> {
    vec![
        // normal: agent_kill stopped it and read back zero processes
        ("kill_confirmed", PriorProcess::Killed, true),
        // boundary: it had already exited before the respawn
        ("already_gone", PriorProcess::Gone, true),
        // the bug: the kill failed, the old copy still runs
        (
            "kill_failed",
            PriorProcess::KillFailed("access denied".to_owned()),
            false,
        ),
        // structurally odd: the kill error text is empty
        (
            "kill_failed_empty_detail",
            PriorProcess::KillFailed(String::new()),
            false,
        ),
        // someone else's thing: registry says dead but the PID runs (maybe reused)
        (
            "pid_reused_or_unconfirmed",
            PriorProcess::Unconfirmed,
            false,
        ),
    ]
}

#[test]
fn respawn_decision_table() {
    let prior_pid = 4242;
    for (name, prior, proceeds) in rows() {
        let decision = respawn_prior_gone(prior_pid, &prior);
        assert_eq!(decision.is_ok(), proceeds, "row {name}");
        if let Err(error) = decision {
            assert!(
                error.message.contains("AGENT_RESPAWN_PRIOR_NOT_STOPPED")
                    && error.message.contains("4242"),
                "row {name}: refusal must name the code and the prior pid, got {}",
                error.message
            );
        }
    }
}
