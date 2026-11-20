//! #173: tracks whether a live SSE stream is attached to a subscription so
//! dashboard-created subscriptions are released when their stream goes away.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(super) struct Attachment {
    attached: AtomicUsize,
    reapable: AtomicBool,
    idle_since: Mutex<Instant>,
}

impl Attachment {
    pub(super) fn new() -> Self {
        Self {
            attached: AtomicUsize::new(0),
            reapable: AtomicBool::new(false),
            idle_since: Mutex::new(Instant::now()),
        }
    }

    /// Only ownerless, stream-driven subscriptions (dashboard, bare `/events`)
    /// are reapable. Session-owned and MCP `subscribe` rows keep their
    /// existing explicit-cancel lifecycle.
    pub(super) fn mark_reapable(&self) {
        self.reapable.store(true, Ordering::Release);
    }

    pub(super) fn attach(&self) {
        self.attached.fetch_add(1, Ordering::AcqRel);
    }

    /// Returns true when this was the last stream of a reapable subscription.
    pub(super) fn detach(&self) -> bool {
        let previous = self
            .attached
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_sub(1))
            })
            .unwrap_or(0);
        if previous > 1 {
            return false;
        }
        if let Ok(mut idle_since) = self.idle_since.lock() {
            *idle_since = Instant::now();
        }
        self.reapable.load(Ordering::Acquire)
    }

    /// True while at least one live stream is attached right now.
    pub(super) fn is_attached(&self) -> bool {
        self.attached.load(Ordering::Acquire) > 0
    }

    pub(super) fn is_orphan(&self, now: Instant, grace: Duration) -> bool {
        self.reapable.load(Ordering::Acquire)
            && self.attached.load(Ordering::Acquire) == 0
            && self
                .idle_since
                .lock()
                .is_ok_and(|since| now.saturating_duration_since(*since) >= grace)
    }
}
