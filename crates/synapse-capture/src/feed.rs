//! Shared per-capture frame feed (#111, #110).
//!
//! The capture worker pushes into one bounded MPMC channel. Several readers
//! (concurrent `livecapture next` / `start` callers) used to clone that
//! receiver, so each frame reached exactly one of them and a reader with a
//! newer cursor also discarded frames an older cursor still needed. The feed
//! drains the channel into a small ordered ring and lets every reader pick a
//! frame by sequence without removing it for the others.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crossbeam::channel::{Receiver, RecvTimeoutError};

use crate::CapturedFrame;

/// Frames retained for late readers. Small: each frame owns its CPU pixels.
const FEED_RING_CAPACITY: usize = 4;
/// Longest a waiting reader blocks on the channel before re-checking the ring,
/// so a frame taken by another reader reaches it promptly.
const FEED_POLL_SLICE: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedWaitError {
    /// No qualifying frame arrived within the timeout.
    Timeout,
    /// The capture worker ended; no further frame can ever arrive.
    Disconnected,
}

#[derive(Debug)]
pub struct FrameFeed {
    rx: Receiver<CapturedFrame>,
    ring: Mutex<VecDeque<Arc<CapturedFrame>>>,
    /// Serializes every dequeue from `rx` together with the publication of the
    /// dequeued frame into `ring`. Without it a reader could take the final
    /// frame off the channel while another reader already sees the channel
    /// disconnected and gives up before the frame is published (#111).
    pump: Mutex<()>,
    joiners: AtomicUsize,
}

impl FrameFeed {
    #[must_use]
    pub fn new(rx: Receiver<CapturedFrame>) -> Self {
        Self {
            rx,
            ring: Mutex::new(VecDeque::with_capacity(FEED_RING_CAPACITY)),
            pump: Mutex::new(()),
            joiners: AtomicUsize::new(0),
        }
    }

    /// Records that another caller adopted this running capture (an idempotent
    /// repeated start). The creating start must then not stop it on failure.
    pub fn join(&self) {
        self.joiners.fetch_add(1, Ordering::AcqRel);
    }

    #[must_use]
    pub fn has_joiners(&self) -> bool {
        self.joiners.load(Ordering::Acquire) > 0
    }

    fn push(&self, frame: CapturedFrame) {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        let position = ring.partition_point(|held| held.frame_seq < frame.frame_seq);
        if ring
            .get(position)
            .is_some_and(|held| held.frame_seq == frame.frame_seq)
        {
            return;
        }
        ring.insert(position, Arc::new(frame));
        while ring.len() > FEED_RING_CAPACITY {
            ring.pop_front();
        }
    }

    fn drain_channel(&self) {
        let _pump = self.pump.lock().unwrap_or_else(PoisonError::into_inner);
        while let Ok(frame) = self.rx.try_recv() {
            self.push(frame);
        }
    }

    /// Blocks up to `slice` for one frame and publishes it before releasing
    /// the pump, so a concurrent reader that observes a disconnected channel
    /// afterwards always finds the frame in the ring.
    fn pump_once(&self, slice: Duration) -> Result<(), RecvTimeoutError> {
        let _pump = self.pump.lock().unwrap_or_else(PoisonError::into_inner);
        self.rx.recv_timeout(slice).map(|frame| self.push(frame))
    }

    /// Newest retained frame with `frame_seq > floor` (any frame when `None`).
    /// Non-blocking.
    #[must_use]
    pub fn try_latest_after(&self, floor: Option<u64>) -> Option<Arc<CapturedFrame>> {
        self.drain_channel();
        let ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        ring.back()
            .filter(|frame| floor.is_none_or(|seq| frame.frame_seq > seq))
            .cloned()
    }

    fn oldest_after(&self, after: Option<u64>) -> Option<Arc<CapturedFrame>> {
        self.drain_channel();
        let ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        ring.iter()
            .find(|frame| after.is_none_or(|seq| frame.frame_seq > seq))
            .cloned()
    }

    /// Waits for the oldest frame with `frame_seq > after` (any frame when
    /// `None`). Frames are never removed for other readers.
    ///
    /// # Errors
    ///
    /// [`FeedWaitError::Timeout`] when nothing qualifying arrives in time;
    /// [`FeedWaitError::Disconnected`] once the capture worker has ended and no
    /// retained frame qualifies.
    pub fn wait_after(
        &self,
        after: Option<u64>,
        timeout: Duration,
    ) -> Result<Arc<CapturedFrame>, FeedWaitError> {
        let started = Instant::now();
        loop {
            if let Some(frame) = self.oldest_after(after) {
                return Ok(frame);
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return Err(FeedWaitError::Timeout);
            }
            match self.pump_once(timeout.saturating_sub(elapsed).min(FEED_POLL_SLICE)) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return self.oldest_after(after).ok_or(FeedWaitError::Disconnected);
                }
            }
        }
    }
}
