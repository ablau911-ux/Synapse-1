use super::{SessionActivity, seat_owner_idle_expired, seat_owner_idle_limit_ms};

const MINUTE_MS: u64 = 60_000;

fn session(id: &str, spawned: bool, closed: bool, idle_ms: u64) -> SessionActivity<'_> {
    SessionActivity {
        session_id: id,
        is_spawned_agent: spawned,
        is_closed: closed,
        last_seen_ms_ago: idle_ms,
    }
}

#[test]
fn must_pass_default_limit_is_thirty_minutes_and_zero_disables() {
    assert_eq!(seat_owner_idle_limit_ms(None), Some(30 * MINUTE_MS));
    assert_eq!(seat_owner_idle_limit_ms(Some(" ")), Some(30 * MINUTE_MS));
    assert_eq!(seat_owner_idle_limit_ms(Some("120")), Some(120_000));
    assert_eq!(seat_owner_idle_limit_ms(Some("0")), None);
    assert_eq!(seat_owner_idle_limit_ms(Some("soon")), Some(30 * MINUTE_MS));
}

#[test]
fn must_pass_crashed_client_seat_owner_is_reclaimed_after_the_limit() {
    let activity = [session("crashed", false, false, 31 * MINUTE_MS)];
    assert!(seat_owner_idle_expired("crashed", activity, 30 * MINUTE_MS));
}

#[test]
fn must_fail_recently_active_owner_keeps_its_seat() {
    let activity = [session("busy", false, false, 5 * MINUTE_MS)];
    assert!(!seat_owner_idle_expired("busy", activity, 30 * MINUTE_MS));
}

#[test]
fn must_fail_owner_idle_exactly_at_limit_keeps_its_seat() {
    let at_limit = [session("edge", false, false, 30 * MINUTE_MS)];
    assert!(!seat_owner_idle_expired("edge", at_limit, 30 * MINUTE_MS));
    let one_ms_over = [session("edge", false, false, 30 * MINUTE_MS + 1)];
    assert!(seat_owner_idle_expired("edge", one_ms_over, 30 * MINUTE_MS));
    let one_ms_under = [session("edge", false, false, 30 * MINUTE_MS - 1)];
    assert!(!seat_owner_idle_expired(
        "edge",
        one_ms_under,
        30 * MINUTE_MS
    ));
}

#[test]
fn must_fail_spawned_agent_owner_is_never_reclaimed_by_mcp_idleness() {
    let activity = [session("agent", true, false, 600 * MINUTE_MS)];
    assert!(!seat_owner_idle_expired("agent", activity, 30 * MINUTE_MS));
}

#[test]
fn must_fail_closed_or_unknown_owner_is_left_to_the_ordinary_scan() {
    let closed = [session("gone", false, true, 600 * MINUTE_MS)];
    assert!(!seat_owner_idle_expired("gone", closed, 30 * MINUTE_MS));
    let other = [session("someone-else", false, false, 600 * MINUTE_MS)];
    assert!(!seat_owner_idle_expired("unknown", other, 30 * MINUTE_MS));
}
