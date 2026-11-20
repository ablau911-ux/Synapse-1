//! #2349: a UI Automation pattern's state is read again, at short steps, until
//! it leaves the state it had before the pattern call (Firefox updates
//! `ToggleState` a moment after `Toggle` returns). Pure: the read and the wait
//! are closures; the UIA calls and the sleep stay with the caller.

use std::time::Duration;

use crate::A11yResult;

#[cfg(test)]
#[path = "state_settle_table_2349.rs"]
mod state_settle_table_2349;

/// Longest time a readback waits for the state to change.
const SETTLE_BUDGET: Duration = Duration::from_millis(250);
/// Pause between two reads.
const SETTLE_STEP: Duration = Duration::from_millis(25);

/// The readback of one pattern call: the state before the call, how to read
/// the state now, and how to wait one step.
pub(super) struct StateSettle<S, R, W> {
    before: S,
    read: R,
    wait: W,
}

impl<S, R, W> StateSettle<S, R, W>
where
    S: PartialEq,
    R: FnMut() -> A11yResult<S>,
    W: FnMut(Duration),
{
    /// `before` is the state the readback must see changed; `read` reads the
    /// state now; `wait` pauses for the given step.
    pub(super) const fn new(before: S, read: R, wait: W) -> Self {
        Self { before, read, wait }
    }

    /// Reads the state until it differs from `before` or the budget ends, and
    /// returns the last state read. A failed read is returned at once.
    pub(super) fn until_changed(mut self) -> A11yResult<S> {
        let mut state = (self.read)()?;
        let mut waited = Duration::ZERO;
        while state == self.before && waited < SETTLE_BUDGET {
            (self.wait)(SETTLE_STEP);
            waited = waited.saturating_add(SETTLE_STEP);
            state = (self.read)()?;
        }
        Ok(state)
    }
}
