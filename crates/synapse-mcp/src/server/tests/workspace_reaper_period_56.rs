//! An invalid reap interval must fall back to the default, never abort startup (#56).

use std::time::Duration;

use super::WorkspaceReaper;

#[test]
fn must_pass_valid_period_is_parsed_with_surrounding_whitespace() {
    assert_eq!(
        WorkspaceReaper::parse_period(" 2500 "),
        Some(Duration::from_millis(2500))
    );
}

#[test]
fn must_fail_invalid_periods_are_rejected_so_the_default_applies() {
    for raw in ["", "0", "-5", "abc", "1.5", "10 minutes"] {
        assert_eq!(WorkspaceReaper::parse_period(raw), None, "raw={raw:?}");
    }
}
