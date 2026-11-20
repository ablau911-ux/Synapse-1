//! Bounded event log for the recording backend (#196).

use std::collections::VecDeque;

use super::RecordedInput;

/// Most events the recording backend retains. Older events are evicted so a
/// long-running env-gated recording daemon cannot grow without limit.
pub const MAX_RECORDED_EVENTS: usize = 16_384;

/// Ring of the most recent events plus a monotonic total, so callers can keep
/// "events recorded so far" cursors that stay valid across evictions.
#[derive(Clone, Debug, Default)]
pub(super) struct EventLog {
    retained: VecDeque<RecordedInput>,
    evicted: usize,
}

impl EventLog {
    pub(super) fn push(&mut self, event: RecordedInput) {
        if self.retained.len() >= MAX_RECORDED_EVENTS {
            self.retained.pop_front();
            self.evicted = self.evicted.saturating_add(1);
        }
        self.retained.push_back(event);
    }

    /// Events recorded since creation, including evicted ones.
    pub(super) fn total(&self) -> usize {
        self.evicted.saturating_add(self.retained.len())
    }

    pub(super) fn retained(&self) -> Vec<RecordedInput> {
        self.retained.iter().cloned().collect()
    }

    /// Retained events recorded after the first `total_before` events.
    pub(super) fn since(&self, total_before: usize) -> Vec<RecordedInput> {
        let skip = total_before.saturating_sub(self.evicted);
        self.retained.iter().skip(skip).cloned().collect()
    }
}
