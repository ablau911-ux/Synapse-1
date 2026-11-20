//! #107(a) frozen behaviour table (manager-owned, work method v3). Codex and
//! Opus never edit this file. One case per L row of the #107(a) table: a
//! caret-type control clicked by element id is posted to its child window when
//! this session's claimed target is not the OS foreground; it never falls to
//! SendInput there. The foreground case (#189/#194) stays as it is.

use super::metadata::{CoordinateTier, coordinate_tier};

const ROOT: i64 = 0x0A_0A0A;
const OTHER_AGENT: i64 = 0x0B_0B0B;
const OWNER: i64 = 0x0C_0C0C;

#[test]
fn coordinate_tier_table_107a() {
    // (row, caret required, this session's claimed root, OS foreground root, expected tier)
    type Row = (&'static str, bool, Option<i64>, Option<i64>, CoordinateTier);
    let rows: [Row; 7] = [
        (
            "107i_caret_claimed_target_behind_owner_window_is_posted",
            true,
            Some(ROOT),
            Some(OWNER),
            CoordinateTier::PostMessage,
        ),
        (
            "107i_second_actor_window_in_front_caret_claimed_target_is_posted",
            true,
            Some(ROOT),
            Some(OTHER_AGENT),
            CoordinateTier::PostMessage,
        ),
        (
            "107i_empty_no_foreground_window_locked_desktop_caret_claimed_is_posted",
            true,
            Some(ROOT),
            None,
            CoordinateTier::PostMessage,
        ),
        (
            "107i_neighbour_caret_claimed_target_is_foreground_keeps_foreground_tier",
            true,
            Some(ROOT),
            Some(ROOT),
            CoordinateTier::Foreground,
        ),
        (
            "107i_neighbour_no_caret_is_posted",
            false,
            Some(ROOT),
            Some(OWNER),
            CoordinateTier::PostMessage,
        ),
        (
            "107i_neighbour_no_caret_no_claim_is_posted",
            false,
            None,
            Some(OWNER),
            CoordinateTier::PostMessage,
        ),
        (
            "107i_neighbour_caret_profile_route_without_claim_keeps_foreground_tier",
            true,
            None,
            Some(OWNER),
            CoordinateTier::Foreground,
        ),
    ];
    let mut failures = Vec::new();
    for (row, caret_required, claimed_root, foreground_root, expected) in rows {
        let got = coordinate_tier(caret_required, claimed_root, foreground_root);
        if got != expected {
            failures.push(format!("row {row}: tier {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#107(a) tier rows failed:\n{}",
        failures.join("\n")
    );
}
