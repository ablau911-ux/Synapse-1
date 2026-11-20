//! FROZEN behaviour table of roadmap item #607 (memory step 4, "your
//! routines"), rows C1-C11 (`EpisodeCatchUp`). Written from the table before
//! the code. Coders never edit this file; a wrong row is reported to the
//! manager.
//! Place: crates/synapse-mcp/src/m3/episode_catch_up_table_607.rs (registered
//! by the `#[cfg(test)] #[path]` lines at the top of episode_catch_up.rs).
//! One test per row, named after the row.
//! A "reply" is what one `episode_segment` call answered; only
//! `next_start_ts_ns`, `stopped_because` and `episodes_written` matter here.
//! CONTROL lines stop a stub that never asks for a call (today: nothing cuts
//! the timeline before mining) or one that never stops from passing a row.

use super::EpisodeCatchUp;
use crate::m3::episodes::{EpisodeSegmentParams, EpisodeSegmentResponse};

/// Three timestamps one day apart (2025-11-18 09:00 UTC and the next two days).
const DAY1: u64 = 1_763_456_400_000_000_000;
const DAY2: u64 = DAY1 + 86_400_000_000_000;
const DAY3: u64 = DAY2 + 86_400_000_000_000;

fn reply(
    next_start_ts_ns: Option<u64>,
    stopped_because: &str,
    episodes_written: u64,
) -> EpisodeSegmentResponse {
    EpisodeSegmentResponse {
        range_start_ns: 0,
        range_end_ns: 0,
        days_processed: 1,
        scanned_rows: 0,
        invalid_rows: 0,
        ignored_agent_rows: 0,
        payload_anomalies: 0,
        episodes_written,
        episodes_deleted: 0,
        dry_run: false,
        days: Vec::new(),
        next_start_ts_ns,
        stopped_because: stopped_because.to_owned(),
    }
}

/// The next call; the row fails when there is none.
fn call(catch_up: &EpisodeCatchUp, row: &str) -> EpisodeSegmentParams {
    match catch_up.next_call() {
        Some(params) => params,
        None => panic!("{row}: a segment call is expected, got none (the timeline is never cut)"),
    }
}

/// Every call writes episodes of human activity only, up to now.
fn assert_writes_human_up_to_now(params: &EpisodeSegmentParams, row: &str) {
    assert_eq!(params.end_ts_ns, None, "{row}: up to now");
    assert!(
        !params.include_agent_activity,
        "{row}: human activity only (the miner's default)"
    );
    assert!(
        !params.dry_run,
        "{row}: the episodes are written, not previewed"
    );
}

#[test]
fn c1_no_episode_stored_cuts_the_whole_timeline_up_to_now() {
    let catch_up = EpisodeCatchUp::resume_from(None);
    let params = call(&catch_up, "C1");
    assert_eq!(params.start_ts_ns, None, "C1: from the first timeline row");
    assert_writes_human_up_to_now(&params, "C1");
}

#[test]
fn c2_newest_episode_day_is_cut_again() {
    let catch_up = EpisodeCatchUp::resume_from(Some(DAY2));
    let params = call(&catch_up, "C2");
    assert_eq!(
        params.start_ts_ns,
        Some(DAY2),
        "C2: from the newest stored episode (its local day is cut again)"
    );
    assert_writes_human_up_to_now(&params, "C2");
}

#[test]
fn c3_a_paused_reply_continues_where_it_paused() {
    for stopped_because in ["day_cap_reached", "scan_budget_exhausted"] {
        let mut catch_up = EpisodeCatchUp::resume_from(Some(DAY1));
        let _ = call(&catch_up, "C3 control");
        catch_up.record(&reply(Some(DAY3), stopped_because, 3));
        let params = call(&catch_up, "C3");
        assert_eq!(
            params.start_ts_ns,
            Some(DAY3),
            "C3 ({stopped_because}): the next call starts where the reply paused"
        );
        assert_writes_human_up_to_now(&params, "C3");
    }
}

#[test]
fn c4_a_complete_reply_ends_the_catch_up() {
    let mut catch_up = EpisodeCatchUp::resume_from(Some(DAY1));
    let _ = call(&catch_up, "C4 control");
    catch_up.record(&reply(None, "range_complete", 4));
    assert!(
        catch_up.next_call().is_none(),
        "C4: range complete = up to date, no further call"
    );
}

#[test]
fn c5_an_empty_timeline_ends_the_catch_up() {
    let mut catch_up = EpisodeCatchUp::resume_from(None);
    let _ = call(&catch_up, "C5 control");
    catch_up.record(&reply(None, "empty_timeline", 0));
    assert!(
        catch_up.next_call().is_none(),
        "C5: empty timeline = nothing to cut, no further call"
    );
}

#[test]
fn c6_a_reply_that_does_not_move_forward_stops_the_loop() {
    let mut stuck = EpisodeCatchUp::resume_from(Some(DAY2));
    let _ = call(&stuck, "C6 control");
    stuck.record(&reply(Some(DAY2), "scan_budget_exhausted", 0));
    assert!(
        stuck.next_call().is_none(),
        "C6: a pause at the same start would loop for ever: stop"
    );

    let mut moving = EpisodeCatchUp::resume_from(Some(DAY2));
    moving.record(&reply(Some(DAY3), "scan_budget_exhausted", 0));
    assert_eq!(
        call(&moving, "C6 control").start_ts_ns,
        Some(DAY3),
        "C6 control: a pause further on continues"
    );
}

#[test]
fn c7_a_reply_that_goes_back_stops_the_loop() {
    let mut catch_up = EpisodeCatchUp::resume_from(Some(DAY2));
    let _ = call(&catch_up, "C7 control");
    catch_up.record(&reply(Some(DAY1), "day_cap_reached", 0));
    assert!(
        catch_up.next_call().is_none(),
        "C7: a pause before the current start is not progress: stop"
    );
}

#[test]
fn c8_from_the_first_timeline_row_any_pause_is_progress() {
    let mut catch_up = EpisodeCatchUp::resume_from(None);
    catch_up.record(&reply(Some(DAY1), "day_cap_reached", 1));
    assert_eq!(
        call(&catch_up, "C8").start_ts_ns,
        Some(DAY1),
        "C8: no start yet, so the first pause is always further on"
    );
}

#[test]
fn c9_a_whole_catch_up_asks_three_calls_and_counts_their_episodes() {
    let mut catch_up = EpisodeCatchUp::resume_from(None);
    let mut starts = Vec::new();
    let replies = [
        reply(Some(DAY2), "day_cap_reached", 5),
        reply(Some(DAY3), "scan_budget_exhausted", 7),
        reply(None, "range_complete", 2),
    ];
    for answer in &replies {
        starts.push(call(&catch_up, "C9").start_ts_ns);
        catch_up.record(answer);
    }
    assert_eq!(
        starts,
        vec![None, Some(DAY2), Some(DAY3)],
        "C9: the call starts"
    );
    assert!(
        catch_up.next_call().is_none(),
        "C9: up to date after the third reply"
    );
    assert_eq!(
        catch_up.episodes_written(),
        14,
        "C9: 5 + 7 + 2 episodes written"
    );
}

#[test]
fn c10_a_late_reply_after_up_to_date_does_not_restart() {
    let mut catch_up = EpisodeCatchUp::resume_from(Some(DAY1));
    let _ = call(&catch_up, "C10 control");
    catch_up.record(&reply(None, "range_complete", 1));
    catch_up.record(&reply(Some(DAY3), "day_cap_reached", 1));
    assert!(
        catch_up.next_call().is_none(),
        "C10: once up to date, the catch-up stays finished"
    );
}

#[test]
fn c11_an_episode_at_time_zero_is_a_start_not_the_first_row() {
    let catch_up = EpisodeCatchUp::resume_from(Some(0));
    assert_eq!(
        call(&catch_up, "C11").start_ts_ns,
        Some(0),
        "C11: a stored start 0 is kept (only no episode means the first row)"
    );
}
