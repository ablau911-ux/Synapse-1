//! Owner-away mode frozen behaviour table (manager-owned, work method v3).
//! Codex and Opus never edit this file. One case per L row of the owner-away
//! table in the presence lane report (Phase 5, owner decision 2026-11-19).

use super::{
    DEFAULT_QUIET_WINDOW_MS, OwnerPresence, PHYSICAL_MOVE_COALESCE_MS, PresenceInput, admission,
    owner_presence, owner_returned, physical_move_due, quiet_window_from,
};
use synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID;
use synapse_core::error_codes::{USER_ACTIVE, USER_ACTIVITY_UNKNOWN, USER_WORKSTATION_LOCKED};

const QUIET: u64 = 60_000;
const MS: u64 = 1_000_000;

const fn input(
    monitoring: bool,
    locked: bool,
    last_physical_age_ms: Option<u64>,
    monitor_age_ms: Option<u64>,
) -> PresenceInput {
    PresenceInput {
        monitoring,
        locked,
        last_physical_age_ms,
        monitor_age_ms,
    }
}

const fn active(retry_after_ms: u64) -> OwnerPresence {
    OwnerPresence::Active { retry_after_ms }
}

#[test]
fn owner_presence_table_away() {
    let mut failures = Vec::new();

    // (row, reading, quiet window, expected verdict)
    let presence_rows: [(&str, PresenceInput, u64, OwnerPresence); 14] = [
        (
            "away01_normal_idle_past_quiet_window_is_away",
            input(true, false, Some(61_000), Some(600_000)),
            QUIET,
            OwnerPresence::Away,
        ),
        (
            "away02_boundary_age_equal_quiet_is_away",
            input(true, false, Some(QUIET), Some(600_000)),
            QUIET,
            OwnerPresence::Away,
        ),
        (
            "away03_boundary_age_one_below_quiet_is_active_retry_1",
            input(true, false, Some(QUIET - 1), Some(600_000)),
            QUIET,
            active(1),
        ),
        (
            "away04_active_names_retry_time",
            input(true, false, Some(5_000), Some(600_000)),
            QUIET,
            active(55_000),
        ),
        (
            "away05_empty_no_input_monitor_older_than_quiet_is_away",
            input(true, false, None, Some(61_000)),
            QUIET,
            OwnerPresence::Away,
        ),
        (
            "away06_restart_no_input_young_monitor_is_active_until_quiet",
            input(true, false, None, Some(10_000)),
            QUIET,
            active(50_000),
        ),
        (
            "away07_empty_no_input_monitor_age_unknown_is_unknown",
            input(true, false, None, None),
            QUIET,
            OwnerPresence::Unknown,
        ),
        (
            "away08_monitoring_off_is_unknown",
            input(false, false, Some(600_000), Some(600_000)),
            QUIET,
            OwnerPresence::Unknown,
        ),
        (
            "away09_locked_long_idle_is_locked",
            input(true, true, Some(u64::MAX), Some(u64::MAX)),
            QUIET,
            OwnerPresence::Locked,
        ),
        (
            "away10_locked_monitoring_off_is_locked",
            input(false, true, None, None),
            QUIET,
            OwnerPresence::Locked,
        ),
        (
            "away11_gate_off_owner_active_is_away",
            input(true, false, Some(5), Some(600_000)),
            0,
            OwnerPresence::Away,
        ),
        (
            "away12_gate_off_locked_is_still_locked",
            input(true, true, Some(5), Some(600_000)),
            0,
            OwnerPresence::Locked,
        ),
        (
            "away13_hostile_max_quiet_no_overflow",
            input(true, false, Some(5), Some(600_000)),
            u64::MAX,
            active(u64::MAX - 5),
        ),
        (
            "away14_hostile_max_age_is_away",
            input(true, false, Some(u64::MAX), Some(u64::MAX)),
            QUIET,
            OwnerPresence::Away,
        ),
    ];
    for (row, reading, quiet_ms, expected) in presence_rows {
        let got = owner_presence(reading, quiet_ms);
        if got != expected {
            failures.push(format!(
                "row {row}: presence {got:?}, expected {expected:?}"
            ));
        }
    }

    // (row, verdict, owner approved, expected refusal code; None = admitted)
    let admission_rows: [(&str, OwnerPresence, bool, Option<&str>); 7] = [
        ("admit01_away_is_admitted", OwnerPresence::Away, false, None),
        (
            "admit02_active_is_user_active",
            active(10),
            false,
            Some(USER_ACTIVE),
        ),
        (
            "admit03_unknown_is_activity_unknown",
            OwnerPresence::Unknown,
            false,
            Some(USER_ACTIVITY_UNKNOWN),
        ),
        (
            "admit04_locked_is_workstation_locked",
            OwnerPresence::Locked,
            false,
            Some(USER_WORKSTATION_LOCKED),
        ),
        (
            "admit05_neighbour_approved_active_is_admitted",
            active(10),
            true,
            None,
        ),
        (
            "admit06_approved_locked_is_still_workstation_locked",
            OwnerPresence::Locked,
            true,
            Some(USER_WORKSTATION_LOCKED),
        ),
        (
            "admit07_neighbour_approved_unknown_is_admitted",
            OwnerPresence::Unknown,
            true,
            None,
        ),
    ];
    for (row, presence, approved, expected) in admission_rows {
        let got = admission(presence, approved).err();
        if got != expected {
            failures.push(format!("row {row}: refusal {got:?}, expected {expected:?}"));
        }
    }

    // (row, event injected, lease holder, expected "owner returned")
    let return_rows: [(&str, bool, Option<&str>, bool); 6] = [
        (
            "return01_physical_input_while_agent_holds_lease_stops",
            false,
            Some("session-a"),
            true,
        ),
        (
            "return02_neighbour_agent_injected_input_never_counts",
            true,
            Some("session-a"),
            false,
        ),
        (
            "return03_neighbour_physical_input_without_lease_stops_nothing",
            false,
            None,
            false,
        ),
        (
            "return04_operator_hotkey_lease_is_not_an_agent_action",
            false,
            Some(OPERATOR_LEASE_OWNER_SESSION_ID),
            false,
        ),
        (
            "return05_second_actor_other_session_lease_also_stops",
            false,
            Some("session-b"),
            true,
        ),
        (
            "return06_hostile_empty_holder_id_still_stops",
            false,
            Some(""),
            true,
        ),
    ];
    for (row, injected, holder, expected) in return_rows {
        let got = owner_returned(injected, holder);
        if got != expected {
            failures.push(format!("row {row}: returned {got}, expected {expected}"));
        }
    }

    // (row, now ns, last counted ns, expected "counted")
    let gap_ns = PHYSICAL_MOVE_COALESCE_MS * MS;
    let move_rows: [(&str, u64, u64, bool); 4] = [
        ("move01_first_physical_move_counts", 5 * gap_ns, 0, true),
        (
            "move02_move_inside_gap_is_coalesced",
            5 * gap_ns + gap_ns - 1,
            5 * gap_ns,
            false,
        ),
        (
            "move03_boundary_move_at_gap_counts",
            6 * gap_ns,
            5 * gap_ns,
            true,
        ),
        (
            "move04_hostile_clock_backwards_counts",
            gap_ns,
            9 * gap_ns,
            true,
        ),
    ];
    for (row, now_ns, last_ns, expected) in move_rows {
        let got = physical_move_due(now_ns, last_ns, PHYSICAL_MOVE_COALESCE_MS);
        if got != expected {
            failures.push(format!("row {row}: counted {got}, expected {expected}"));
        }
    }

    // (row, raw config, expected quiet window)
    let quiet_rows: [(&str, Option<&str>, u64); 6] = [
        ("quiet01_unset_is_default", None, DEFAULT_QUIET_WINDOW_MS),
        (
            "quiet02_hostile_text_is_default",
            Some("abc"),
            DEFAULT_QUIET_WINDOW_MS,
        ),
        (
            "quiet03_hostile_negative_is_default",
            Some("-5"),
            DEFAULT_QUIET_WINDOW_MS,
        ),
        ("quiet04_padded_number_is_read", Some(" 1500 "), 1_500),
        ("quiet05_zero_turns_activity_check_off", Some("0"), 0),
        (
            "quiet06_boundary_max_is_kept",
            Some("18446744073709551615"),
            u64::MAX,
        ),
    ];
    for (row, raw, expected) in quiet_rows {
        let got = quiet_window_from(raw);
        if got != expected {
            failures.push(format!("row {row}: quiet {got}, expected {expected}"));
        }
    }

    assert!(
        failures.is_empty(),
        "owner-away table rows failed:\n{}",
        failures.join("\n")
    );
}
