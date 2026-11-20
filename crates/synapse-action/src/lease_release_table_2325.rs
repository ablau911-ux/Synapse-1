//! #2325 frozen behaviour table (manager-owned, work method v3). Codex and the
//! review agent never edit this file. One case per L row of the table in Gitea
//! issue #2325.

use super::{LeaseReleaseDecision, lease_release_decision};

#[test]
fn lease_release_decision_table_2325() {
    let rows: [(&str, Option<&str>, &str, LeaseReleaseDecision); 7] = [
        (
            "2325a_holder_releases",
            Some("s1"),
            "s1",
            LeaseReleaseDecision::Release,
        ),
        (
            "2325b_nobody_holds_is_already_released",
            None,
            "s1",
            LeaseReleaseDecision::AlreadyReleased,
        ),
        (
            "2325c_other_session_holds_is_refused",
            Some("s2"),
            "s1",
            LeaseReleaseDecision::NotHolder,
        ),
        (
            "2325d_empty_session_id_with_no_holder_is_already_released",
            None,
            "",
            LeaseReleaseDecision::AlreadyReleased,
        ),
        (
            "2325d_empty_session_id_never_matches_a_holder",
            Some("s2"),
            "",
            LeaseReleaseDecision::NotHolder,
        ),
        (
            "2325d_hostile_prefix_is_not_the_holder",
            Some("s1"),
            "s1 ",
            LeaseReleaseDecision::NotHolder,
        ),
        (
            "2325d_case_differs_is_not_the_holder",
            Some("S1"),
            "s1",
            LeaseReleaseDecision::NotHolder,
        ),
    ];
    let mut failures = Vec::new();
    for (row, holder, session, expected) in rows {
        let got = lease_release_decision(holder, session);
        if got != expected {
            failures.push(format!("row {row}: {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2325 table rows failed:\n{}",
        failures.join("\n")
    );
}
