//! #147 review follow-up: idle reclaim picks owners whose seats should close
//! (the session itself is never torn down), and an open SSE stream counts as
//! activity.

use std::collections::BTreeSet;

use super::{SessionActivity, idle_seat_owners};

const MINUTE_MS: u64 = 60_000;
const LIMIT_MS: u64 = 30 * MINUTE_MS;

fn session(id: &str, spawned: bool, idle_ms: u64) -> SessionActivity<'_> {
    SessionActivity {
        session_id: id,
        is_spawned_agent: spawned,
        is_closed: false,
        last_seen_ms_ago: idle_ms,
    }
}

#[test]
fn must_pass_idle_human_owner_is_selected_for_seat_close_only() {
    let activity = [session("human", false, 45 * MINUTE_MS)];
    let owners = idle_seat_owners(["human"], &activity, &BTreeSet::new(), LIMIT_MS);
    assert_eq!(owners, vec!["human".to_owned()]);
}

#[test]
fn must_fail_open_sse_stream_counts_as_activity() {
    let activity = [session("streaming", false, 45 * MINUTE_MS)];
    let streaming = BTreeSet::from(["streaming".to_owned()]);
    assert!(idle_seat_owners(["streaming"], &activity, &streaming, LIMIT_MS).is_empty());
}

#[test]
fn must_fail_active_spawned_and_unknown_owners_are_not_selected() {
    let activity = [
        session("busy", false, 5 * MINUTE_MS),
        session("agent", true, 600 * MINUTE_MS),
    ];
    let owners = idle_seat_owners(
        ["busy", "agent", "unknown"],
        &activity,
        &BTreeSet::new(),
        LIMIT_MS,
    );
    assert!(owners.is_empty());
}

#[test]
fn must_pass_each_owner_is_reported_once_and_only_idle_ones() {
    let activity = [
        session("a", false, 31 * MINUTE_MS),
        session("b", false, MINUTE_MS),
    ];
    let owners = idle_seat_owners(["a", "a", "b"], &activity, &BTreeSet::new(), LIMIT_MS);
    assert_eq!(owners, vec!["a".to_owned()]);
}
