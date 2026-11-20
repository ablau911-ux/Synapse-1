//! Regression test for #111 (re-review): the final frame of a stopping capture
//! must reach every concurrent reader. A reader that dequeues the frame from
//! the channel but has not yet published it to the ring must not let another
//! reader observe `Disconnected` and fail while a qualifying frame exists.

use std::{
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel;
use synapse_capture::{CapturedFrame, CapturedFrameBuffer, DxgiFormat, FeedWaitError, FrameFeed};

fn frame(seq: u64) -> CapturedFrame {
    CapturedFrame {
        pixels: CapturedFrameBuffer {
            bytes: vec![0; 4],
            row_stride_bytes: 4,
            bytes_per_pixel: 4,
        },
        width: 1,
        height: 1,
        format: DxgiFormat::Bgra8,
        captured_at: Instant::now(),
        frame_seq: seq,
        dirty_region: None,
    }
}

#[test]
fn must_pass_final_frame_of_stopped_capture_reaches_both_concurrent_readers() {
    for round in 0..3000_u64 {
        let (tx, rx) = channel::bounded(2);
        let feed = Arc::new(FrameFeed::new(rx));
        tx.send(frame(round + 1)).expect("send");
        drop(tx);
        let gate = Arc::new(Barrier::new(2));
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let feed = Arc::clone(&feed);
                let gate = Arc::clone(&gate);
                thread::spawn(move || {
                    gate.wait();
                    feed.wait_after(None, Duration::from_secs(2))
                        .map(|f| f.frame_seq)
                })
            })
            .collect();
        for reader in readers {
            let outcome = reader.join().expect("reader thread");
            assert_eq!(
                outcome,
                Ok(round + 1),
                "round {round}: a reader lost the final frame"
            );
        }
    }
}

#[test]
fn must_fail_readers_past_the_final_frame_of_a_stopped_capture_get_disconnected() {
    let (tx, rx) = channel::bounded(2);
    let feed = Arc::new(FrameFeed::new(rx));
    tx.send(frame(7)).expect("send");
    drop(tx);
    let gate = Arc::new(Barrier::new(2));
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let feed = Arc::clone(&feed);
            let gate = Arc::clone(&gate);
            thread::spawn(move || {
                gate.wait();
                feed.wait_after(Some(7), Duration::from_secs(2))
                    .map(|f| f.frame_seq)
            })
        })
        .collect();
    for reader in readers {
        let outcome = reader.join().expect("reader thread");
        assert_eq!(
            outcome,
            Err(FeedWaitError::Disconnected),
            "a cursor at the final frame must not re-read it or time out"
        );
    }
}

#[test]
fn must_fail_stopped_capture_without_frames_is_disconnected_and_live_silent_capture_times_out() {
    let (stopped_tx, stopped_rx) = channel::bounded::<CapturedFrame>(2);
    drop(stopped_tx);
    let stopped = FrameFeed::new(stopped_rx);
    assert_eq!(
        stopped
            .wait_after(None, Duration::from_secs(2))
            .map(|f| f.frame_seq),
        Err(FeedWaitError::Disconnected)
    );
    assert!(stopped.try_latest_after(None).is_none());

    let (live_tx, live_rx) = channel::bounded::<CapturedFrame>(2);
    let live = FrameFeed::new(live_rx);
    let started = Instant::now();
    assert_eq!(
        live.wait_after(None, Duration::from_millis(100))
            .map(|f| f.frame_seq),
        Err(FeedWaitError::Timeout)
    );
    assert!(started.elapsed() >= Duration::from_millis(100));
    drop(live_tx);
}
