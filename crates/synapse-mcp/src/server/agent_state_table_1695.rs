//! #1695 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the table in the swarm report
//! for Gitea issue #1695 (upstream #1594): an agent marked dead only because it
//! went silent comes back on live evidence; a busy agent gets a longer
//! silence deadline; a confirmed death stays final.

use super::{
    AgentLifecycleState, AgentStateTracker, PostDeathEvent, classify_post_death_event,
    unprobeable_dead_after_ms,
};
use synapse_core::{AgentEventKind, AgentEventRecord};

const INFERRED: Option<&str> = Some("unprobeable_silent_ended");
const BASE_MS: u64 = 1_800_000;
const STUCK_MS: u64 = 120_000;
const T0: u64 = 1_000_000;

#[test]
fn post_death_event_table_1695() {
    use AgentEventKind as K;
    use PostDeathEvent as P;
    // (row, death reason, event kind, event ms, last pre-death event ms, expected)
    let rows: [(&str, Option<&str>, K, u64, u64, P); 14] = [
        (
            "1695a_inferred_death_tool_started_revives",
            INFERRED,
            K::ToolCallStarted,
            2_000,
            1_000,
            P::Revive,
        ),
        (
            "1695a_inferred_death_tool_finished_revives",
            INFERRED,
            K::ToolCallFinished,
            2_000,
            1_000,
            P::Revive,
        ),
        (
            "1695a_inferred_death_turn_started_revives",
            INFERRED,
            K::TurnStarted,
            2_000,
            1_000,
            P::Revive,
        ),
        (
            "1695a_inferred_death_turn_finished_revives",
            INFERRED,
            K::TurnFinished,
            2_000,
            1_000,
            P::Revive,
        ),
        (
            "1695b_empty_inferred_death_mailbox_traffic_is_not_evidence",
            INFERRED,
            K::MessageReceived,
            2_000,
            1_000,
            P::Ignore,
        ),
        (
            "1695b_empty_inferred_death_lease_traffic_is_not_evidence",
            INFERRED,
            K::LeaseAcquired,
            2_000,
            1_000,
            P::Ignore,
        ),
        (
            "1695b_empty_death_without_reason_stays_dead",
            None,
            K::ToolCallStarted,
            2_000,
            1_000,
            P::Ignore,
        ),
        (
            "1695d_hostile_event_same_ms_as_last_seen_is_not_new_evidence",
            INFERRED,
            K::ToolCallStarted,
            1_000,
            1_000,
            P::Ignore,
        ),
        (
            "1695d_hostile_event_older_than_last_seen_is_not_new_evidence",
            INFERRED,
            K::TurnStarted,
            999,
            1_000,
            P::Ignore,
        ),
        (
            "1695e_neighbour_killed_never_revives",
            Some("killed"),
            K::ToolCallStarted,
            2_000,
            1_000,
            P::Ignore,
        ),
        (
            "1695e_neighbour_process_gone_never_revives",
            Some("process_gone_without_exit_event"),
            K::TurnStarted,
            2_000,
            1_000,
            P::Ignore,
        ),
        (
            "1695e_neighbour_process_gone_late_exit_reconciles",
            Some("process_gone_without_exit_event"),
            K::Exited,
            2_000,
            1_000,
            P::ReconcileExit,
        ),
        (
            "1695e_neighbour_spawn_requested_reregisters",
            INFERRED,
            K::SpawnRequested,
            2_000,
            1_000,
            P::Reregister,
        ),
        (
            "1695e_neighbour_inferred_death_then_killed_stays_dead",
            INFERRED,
            K::Killed,
            2_000,
            1_000,
            P::Ignore,
        ),
    ];
    let mut failures = Vec::new();
    for (row, reason, kind, event_ms, last_ms, expected) in rows {
        let got = classify_post_death_event(reason, kind, event_ms, last_ms);
        if got != expected {
            failures.push(format!("row {row}: {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#1695 post-death rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn unprobeable_deadline_table_1695() {
    use AgentEventKind as K;
    // (row, base deadline ms, last event kind, expected deadline ms)
    let rows: [(&str, u64, K, u64); 5] = [
        (
            "1695c_idle_between_turns_keeps_base_deadline",
            BASE_MS,
            K::TurnFinished,
            BASE_MS,
        ),
        (
            "1695c_working_turn_keeps_base_deadline",
            BASE_MS,
            K::TurnStarted,
            BASE_MS,
        ),
        (
            "1695c_finished_tool_keeps_base_deadline",
            BASE_MS,
            K::ToolCallFinished,
            BASE_MS,
        ),
        (
            "1695c_in_flight_tool_gets_four_times_base",
            BASE_MS,
            K::ToolCallStarted,
            4 * BASE_MS,
        ),
        (
            "1695c_hostile_huge_base_saturates_not_overflows",
            u64::MAX,
            K::ToolCallStarted,
            u64::MAX,
        ),
    ];
    let mut failures = Vec::new();
    for (row, base, kind, expected) in rows {
        let got = unprobeable_dead_after_ms(base, kind);
        if got != expected {
            failures.push(format!("row {row}: {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#1695 deadline rows failed:\n{}",
        failures.join("\n")
    );
}

fn ev(kind: AgentEventKind, spawn: &str, at_ms: u64) -> AgentEventRecord {
    let mut record = AgentEventRecord::new(at_ms * 1_000_000, kind);
    record.spawn_id = Some(spawn.to_owned());
    if kind == AgentEventKind::ToolCallStarted {
        record.attributes.tool_name = Some("act_run_shell".to_owned());
        record.payload = serde_json::json!({ "tool_input_sha256": format!("sha256:{at_ms}") });
    }
    record
}

fn state_of(
    tracker: &AgentStateTracker,
    anchor: &str,
    now_ms: u64,
) -> Option<(AgentLifecycleState, Option<String>)> {
    tracker
        .reads(now_ms)
        .into_iter()
        .find(|read| read.anchor == anchor)
        .map(|read| (read.state, read.reason_code))
}

fn sweep(tracker: &mut AgentStateTracker, now_ms: u64) {
    let _transitions = tracker.sweep(now_ms, STUCK_MS, BASE_MS, &|_pid| true);
}

/// Reaps an idle unprobeable agent by silence; returns the sweep time.
fn reaped_idle(tracker: &mut AgentStateTracker, spawn: &str, at_ms: u64) -> u64 {
    tracker.apply_event(&ev(AgentEventKind::SpawnRequested, spawn, at_ms));
    tracker.apply_event(&ev(AgentEventKind::TurnFinished, spawn, at_ms));
    let swept = at_ms + BASE_MS;
    sweep(tracker, swept);
    swept
}

fn expect(failures: &mut Vec<String>, row: &str, ok: bool, detail: String) {
    if !ok {
        failures.push(format!("row {row}: {detail}"));
    }
}

#[test]
fn tracker_liveness_table_1695() {
    use AgentEventKind as K;
    use AgentLifecycleState as S;
    let mut failures = Vec::new();

    {
        let row = "1695t_normal_reaped_idle_agent_revives_on_tool_call";
        let mut tracker = AgentStateTracker::default();
        let swept = reaped_idle(&mut tracker, "agent-spawn-a", T0);
        let dead = state_of(&tracker, "agent-spawn-a", swept);
        expect(
            &mut failures,
            row,
            dead == Some((S::Dead, INFERRED.map(ToOwned::to_owned))),
            format!("precondition dead by silence, got {dead:?}"),
        );
        let transition = tracker.apply_event(&ev(K::ToolCallStarted, "agent-spawn-a", swept + 10));
        let shape = transition.as_ref().map(|t| {
            (
                t.state_from,
                t.state_to,
                t.reason_code.clone(),
                t.evidence["prior_death_reason"]
                    .as_str()
                    .map(ToOwned::to_owned),
                t.evidence["trigger_event_kind"]
                    .as_str()
                    .map(ToOwned::to_owned),
            )
        });
        let want = Some((
            S::Dead,
            S::Working,
            "resurrected_by_live_evidence".to_owned(),
            Some("unprobeable_silent_ended".to_owned()),
            Some("tool_call_started".to_owned()),
        ));
        expect(
            &mut failures,
            row,
            shape == want,
            format!("transition {shape:?}, expected {want:?}"),
        );
        let now = state_of(&tracker, "agent-spawn-a", swept + 10).map(|(state, _)| state);
        expect(
            &mut failures,
            row,
            now == Some(S::Working),
            format!("readback {now:?}, expected Working"),
        );
    }

    {
        let row = "1695t_repeat_reap_revive_reap_revive";
        let mut tracker = AgentStateTracker::default();
        let first = reaped_idle(&mut tracker, "agent-spawn-r", T0);
        let revived_once = tracker
            .apply_event(&ev(K::ToolCallStarted, "agent-spawn-r", first + 10))
            .map(|t| t.state_to);
        tracker.apply_event(&ev(K::TurnFinished, "agent-spawn-r", first + 20));
        let second = first + 20 + BASE_MS;
        sweep(&mut tracker, second);
        let dead_again = state_of(&tracker, "agent-spawn-r", second).map(|(state, _)| state);
        let revived_twice = tracker
            .apply_event(&ev(K::TurnStarted, "agent-spawn-r", second + 10))
            .map(|t| (t.state_to, t.reason_code));
        expect(
            &mut failures,
            row,
            revived_once == Some(S::Working),
            format!("first revive {revived_once:?}"),
        );
        expect(
            &mut failures,
            row,
            dead_again == Some(S::Dead),
            format!("second reap {dead_again:?}"),
        );
        expect(
            &mut failures,
            row,
            revived_twice == Some((S::Working, "resurrected_by_live_evidence".to_owned())),
            format!("second revive {revived_twice:?}"),
        );
    }

    {
        let row = "1695t_second_actor_busy_a_keeps_grace_idle_b_reaped";
        let mut tracker = AgentStateTracker::default();
        tracker.apply_event(&ev(K::SpawnRequested, "agent-spawn-busy", T0));
        tracker.apply_event(&ev(K::ToolCallStarted, "agent-spawn-busy", T0));
        tracker.apply_event(&ev(K::SpawnRequested, "agent-spawn-idle", T0));
        tracker.apply_event(&ev(K::TurnFinished, "agent-spawn-idle", T0));
        sweep(&mut tracker, T0 + BASE_MS);
        let busy = state_of(&tracker, "agent-spawn-busy", T0 + BASE_MS).map(|(state, _)| state);
        let idle = state_of(&tracker, "agent-spawn-idle", T0 + BASE_MS).map(|(state, _)| state);
        expect(
            &mut failures,
            row,
            busy == Some(S::Stuck),
            format!("busy at base deadline {busy:?}, expected Stuck"),
        );
        expect(
            &mut failures,
            row,
            idle == Some(S::Dead),
            format!("idle at base deadline {idle:?}, expected Dead"),
        );
        sweep(&mut tracker, T0 + 4 * BASE_MS - 1);
        let busy_before =
            state_of(&tracker, "agent-spawn-busy", T0 + 4 * BASE_MS - 1).map(|(state, _)| state);
        expect(
            &mut failures,
            row,
            busy_before == Some(S::Stuck),
            format!("busy 1 ms before extended deadline {busy_before:?}, expected Stuck"),
        );
        sweep(&mut tracker, T0 + 4 * BASE_MS);
        let busy_after =
            state_of(&tracker, "agent-spawn-busy", T0 + 4 * BASE_MS).map(|(state, _)| state);
        expect(
            &mut failures,
            row,
            busy_after == Some(S::Dead),
            format!("busy at extended deadline {busy_after:?}, expected Dead"),
        );
    }

    {
        let row = "1695t_someone_else_activity_of_another_agent_does_not_revive";
        let mut tracker = AgentStateTracker::default();
        tracker.apply_event(&ev(K::SpawnRequested, "agent-spawn-other", T0 + BASE_MS));
        let swept = reaped_idle(&mut tracker, "agent-spawn-mine", T0);
        tracker.apply_event(&ev(K::ToolCallStarted, "agent-spawn-other", swept + 10));
        let mine = state_of(&tracker, "agent-spawn-mine", swept + 10).map(|(state, _)| state);
        expect(
            &mut failures,
            row,
            mine == Some(S::Dead),
            format!("dead agent after another agent's activity {mine:?}, expected Dead"),
        );
    }

    {
        let row = "1695t_neighbour_killed_agent_stays_dead_on_late_tool_call";
        let mut tracker = AgentStateTracker::default();
        tracker.apply_event(&ev(K::SpawnRequested, "agent-spawn-k", T0));
        tracker.apply_event(&ev(K::Killed, "agent-spawn-k", T0 + 5));
        let late = tracker
            .apply_event(&ev(K::ToolCallStarted, "agent-spawn-k", T0 + 10))
            .map(|t| t.state_to);
        let now = state_of(&tracker, "agent-spawn-k", T0 + 10);
        expect(
            &mut failures,
            row,
            late.is_none(),
            format!("late tool call transition {late:?}, expected none"),
        );
        expect(
            &mut failures,
            row,
            now == Some((S::Dead, Some("killed".to_owned()))),
            format!("readback {now:?}"),
        );
    }

    {
        let row = "1695t_hostile_replayed_old_event_does_not_revive";
        let mut tracker = AgentStateTracker::default();
        let swept = reaped_idle(&mut tracker, "agent-spawn-h", T0);
        let replay = tracker
            .apply_event(&ev(K::TurnFinished, "agent-spawn-h", T0))
            .map(|t| t.state_to);
        let now = state_of(&tracker, "agent-spawn-h", swept).map(|(state, _)| state);
        expect(
            &mut failures,
            row,
            replay.is_none() && now == Some(S::Dead),
            format!("replay {replay:?}, readback {now:?}"),
        );
    }

    assert!(
        failures.is_empty(),
        "#1695 tracker rows failed:\n{}",
        failures.join("\n")
    );
}
