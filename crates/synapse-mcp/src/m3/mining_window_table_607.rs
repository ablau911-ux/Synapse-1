//! FROZEN behaviour table of roadmap item #607 second part (rolling mining
//! window, `MiningWindow`), rows W1-W7 of the table in issue #2421. Written by
//! the manager from the table before the code. Coders never edit this file; a
//! wrong row is reported to the manager. One test per row, named after the row.
//! CONTROL lines stop a stub that always answers "whole history" or always
//! refuses from passing a row.

use super::{DEFAULT_WINDOW_DAYS, MiningWindow, WINDOW_DAYS_ENV};

const DAY_NS: u64 = 86_400_000_000_000;
/// 2026-11-20 09:00 UTC.
const NOW: u64 = 1_795_165_200_000_000_000;

fn window(raw: Option<&str>, row: &str) -> MiningWindow {
    match MiningWindow::from_env_value(raw) {
        Ok(window) => window,
        Err(error) => panic!("{row}: {raw:?} must be accepted, got the error {error}"),
    }
}

fn refused(raw: &str, row: &str) -> String {
    match MiningWindow::from_env_value(Some(raw)) {
        Ok(window) => panic!(
            "{row}: {raw:?} must be refused, got a window of {} days",
            window.days()
        ),
        Err(error) => error,
    }
}

#[test]
fn w1_not_set_mines_the_last_28_days() {
    let w = window(None, "W1");
    assert_eq!(DEFAULT_WINDOW_DAYS, 28, "W1: the default is four weeks");
    assert_eq!(w.days(), 28, "W1: days");
    assert_eq!(
        w.start_ts_ns(NOW),
        Some(NOW - 28 * DAY_NS),
        "W1: start = now - 28 days"
    );
}

#[test]
fn w2_empty_or_blank_value_means_the_default() {
    for raw in ["", "   ", "\t"] {
        let w = window(Some(raw), "W2");
        assert_eq!(w.start_ts_ns(NOW), Some(NOW - 28 * DAY_NS), "W2: {raw:?}");
    }
}

#[test]
fn w3_zero_mines_the_whole_history() {
    let w = window(Some("0"), "W3");
    assert_eq!(w.days(), 0, "W3: days");
    assert_eq!(
        w.start_ts_ns(NOW),
        None,
        "W3: no start = the whole history (today's behaviour)"
    );
    // CONTROL: a set value is not the whole history.
    assert_eq!(
        window(Some("7"), "W3 control").start_ts_ns(NOW),
        Some(NOW - 7 * DAY_NS),
        "W3 control"
    );
}

#[test]
fn w4_a_number_of_days_sets_the_window_spaces_trimmed() {
    assert_eq!(
        window(Some("7"), "W4").start_ts_ns(NOW),
        Some(NOW - 7 * DAY_NS),
        "W4: 7 days"
    );
    assert_eq!(
        window(Some(" 14 "), "W4").start_ts_ns(NOW),
        Some(NOW - 14 * DAY_NS),
        "W4: \" 14 \""
    );
    assert_eq!(
        window(Some("1"), "W4").start_ts_ns(NOW),
        Some(NOW - DAY_NS),
        "W4: 1 day"
    );
}

#[test]
fn w5_a_window_longer_than_the_clock_starts_at_zero() {
    let w = window(Some("28"), "W5");
    assert_eq!(
        w.start_ts_ns(3 * DAY_NS),
        Some(0),
        "W5: no underflow when now is younger than the window"
    );
    assert_eq!(w.start_ts_ns(0), Some(0), "W5: now = 0");
}

#[test]
fn w6_a_huge_window_saturates_and_never_overflows() {
    // 213,504 days x 86,400e9 ns overflows u64; the start is the epoch, not a wrapped value.
    let w = window(Some("213504"), "W6");
    assert_eq!(w.days(), 213_504, "W6: days");
    assert_eq!(w.start_ts_ns(NOW), Some(0), "W6: saturates at 0");
    assert_eq!(
        window(Some("3650"), "W6").start_ts_ns(NOW),
        Some(NOW - 3_650 * DAY_NS),
        "W6: 10 years still fits"
    );
}

#[test]
fn w7_an_invalid_value_is_refused_and_names_the_variable() {
    for raw in ["-1", "abc", "1.5", "28d", "1 4", "99999999999999999999999"] {
        let error = refused(raw, "W7");
        assert!(
            error.contains(WINDOW_DAYS_ENV),
            "W7: {raw:?}: the error names {WINDOW_DAYS_ENV}: {error}"
        );
        assert!(
            error.contains(raw.trim()),
            "W7: {raw:?}: the error quotes the value: {error}"
        );
    }
    // CONTROL: a valid value is not refused.
    assert_eq!(window(Some("28"), "W7 control").days(), 28, "W7 control");
}
