//! FROZEN behaviour table of Gitea #2349, rows S1-S6. Written from the issue
//! table before the code. Coders never edit this file; a wrong row is
//! reported to the manager. No Windows API: the read and the wait are plain
//! closures that count their calls.

use std::{
    cell::{Cell, RefCell},
    time::Duration,
};

use crate::{A11yError, A11yResult};

use super::StateSettle;

/// A check box state, standing in for any pattern state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Check {
    Off,
    On,
    Mixed,
}

/// One read: a state, or the text of a failed read.
type Read = Result<Check, &'static str>;

struct Row {
    name: &'static str,
    before: Check,
    /// What the 1st, 2nd, ... read returns.
    reads: Vec<Read>,
    /// The returned state, or a text the returned error must contain.
    returned: Read,
    read_calls: usize,
    wait_calls: usize,
}

const STEP: Duration = Duration::from_millis(25);

#[test]
fn state_settle_table_2349() {
    use Check::{Mixed, Off, On};
    let rows = vec![
        Row {
            name: "S1_changed_on_the_first_read",
            before: Off,
            reads: vec![Ok(On)],
            returned: Ok(On),
            read_calls: 1,
            wait_calls: 0,
        },
        Row {
            name: "S2_same_same_changed_on_the_third_read",
            before: Off,
            reads: vec![Ok(Off), Ok(Off), Ok(On)],
            returned: Ok(On),
            read_calls: 3,
            wait_calls: 2,
        },
        Row {
            // 250 ms in steps of 25 ms: 1 + 250 / 25 = 11 reads, 10 waits.
            name: "S3_never_changes_last_state_after_the_budget",
            before: Off,
            reads: vec![Ok(Off); 20],
            returned: Ok(Off),
            read_calls: 11,
            wait_calls: 10,
        },
        Row {
            name: "S4_second_read_fails_error_returned_no_further_read",
            before: Off,
            reads: vec![Ok(Off), Err("S4 second read failed"), Ok(On)],
            returned: Err("S4 second read failed"),
            read_calls: 2,
            wait_calls: 1,
        },
        Row {
            name: "S5_changed_on_the_last_allowed_read",
            before: Off,
            reads: [vec![Ok(Off); 10], vec![Ok(On)]].concat(),
            returned: Ok(On),
            read_calls: 11,
            wait_calls: 10,
        },
        Row {
            name: "S6_changed_to_a_third_state_mixed",
            before: Off,
            reads: vec![Ok(Mixed)],
            returned: Ok(Mixed),
            read_calls: 1,
            wait_calls: 0,
        },
    ];

    let mut failures = Vec::new();
    for row in &rows {
        let read_calls = Cell::new(0_usize);
        let waits = RefCell::new(Vec::new());
        let returned: A11yResult<Check> = StateSettle::new(
            row.before,
            || {
                let index = read_calls.get();
                read_calls.set(index + 1);
                match row.reads.get(index) {
                    Some(Ok(state)) => Ok(*state),
                    Some(Err(text)) => Err(A11yError::internal(*text)),
                    None => Err(A11yError::internal("table: read past the row's list")),
                }
            },
            |step: Duration| waits.borrow_mut().push(step),
        )
        .until_changed();
        let waits = waits.into_inner();
        let returned_as_row = match (&returned, &row.returned) {
            (Ok(state), Ok(expected)) => state == expected,
            (Err(error), Err(text)) => error.to_string().contains(text),
            _ => false,
        };
        let every_wait_is_one_step = waits.iter().all(|step| *step == STEP);
        if !returned_as_row
            || read_calls.get() != row.read_calls
            || waits.len() != row.wait_calls
            || !every_wait_is_one_step
        {
            failures.push(format!(
                "{}: expected {:?} after {} reads and {} waits of 25 ms, observed {:?} after {} reads and waits {:?}",
                row.name,
                row.returned,
                row.read_calls,
                row.wait_calls,
                returned,
                read_calls.get(),
                waits
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
