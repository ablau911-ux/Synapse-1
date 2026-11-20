//! Regression tests for #111 / #110: concurrent readers of one capture must
//! not take frames from each other, a stopped capture must surface as a
//! terminal error instead of a spin, and adopters must be visible to the
//! creating start.

use std::time::{Duration, Instant};

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

const WAIT: Duration = Duration::from_millis(500);

#[test]
fn must_fail_reader_with_newer_cursor_does_not_steal_frames_from_older_cursor() {
    let (tx, rx) = channel::bounded(2);
    let feed = FrameFeed::new(rx);
    tx.send(frame(3)).expect("send");
    tx.send(frame(5)).expect("send");

    let newer = feed.wait_after(Some(4), WAIT).expect("newer reader");
    assert_eq!(newer.frame_seq, 5);
    let older = feed.wait_after(Some(2), WAIT).expect("older reader");
    assert_eq!(
        older.frame_seq, 3,
        "frame 3 must still reach the older cursor"
    );
    let next = feed
        .wait_after(Some(3), WAIT)
        .expect("older reader resumes");
    assert_eq!(next.frame_seq, 5);
}

#[test]
fn must_pass_one_fresh_frame_on_a_static_window_reaches_both_readers() {
    let (tx, rx) = channel::bounded(2);
    let feed = FrameFeed::new(rx);
    tx.send(frame(7)).expect("send");

    let first = feed.wait_after(Some(6), WAIT).expect("first reader");
    let second = feed.wait_after(Some(6), WAIT).expect("second reader");
    assert_eq!((first.frame_seq, second.frame_seq), (7, 7));
}

#[test]
fn must_pass_start_style_read_takes_newest_without_consuming_it_for_next() {
    let (tx, rx) = channel::bounded(2);
    let feed = FrameFeed::new(rx);
    tx.send(frame(1)).expect("send");
    tx.send(frame(2)).expect("send");

    let start = feed.try_latest_after(None).expect("start frame");
    assert_eq!(start.frame_seq, 2);
    assert!(feed.try_latest_after(Some(2)).is_none(), "nothing newer");
    let next = feed
        .wait_after(Some(0), WAIT)
        .expect("next still sees frame 1");
    assert_eq!(next.frame_seq, 1);
}

#[test]
fn must_fail_stopped_capture_is_a_terminal_error_not_a_timeout_spin() {
    let (tx, rx) = channel::bounded::<CapturedFrame>(2);
    let feed = FrameFeed::new(rx);
    drop(tx);

    let started = Instant::now();
    let error = feed
        .wait_after(Some(1), Duration::from_secs(5))
        .expect_err("disconnected capture");
    assert_eq!(error, FeedWaitError::Disconnected);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "must fail fast, took {:?}",
        started.elapsed()
    );
}

#[test]
fn must_pass_retained_frame_is_still_delivered_after_the_capture_stops() {
    let (tx, rx) = channel::bounded(2);
    let feed = FrameFeed::new(rx);
    tx.send(frame(9)).expect("send");
    drop(tx);
    assert_eq!(feed.wait_after(Some(8), WAIT).expect("frame").frame_seq, 9);
}

#[test]
fn must_pass_adopters_are_visible_to_the_creating_start() {
    // The creating start holds the feed; an idempotent repeated start (another
    // caller, another thread) adopts it while the creator is still running.
    // After the adopter is done, the creator's own read must see the adoption,
    // and a frame published to the shared feed still reaches both of them.
    let (tx, rx) = channel::bounded::<CapturedFrame>(2);
    let feed = FrameFeed::new(rx);
    assert!(!feed.has_joiners(), "a fresh capture has no adopters");

    std::thread::scope(|scope| {
        let adopter = scope.spawn(|| {
            feed.join();
            feed.wait_after(Some(0), WAIT).map(|frame| frame.frame_seq)
        });
        tx.send(frame(1)).expect("send");
        let adopted = adopter.join().expect("adopter thread");
        assert_eq!(adopted, Ok(1), "the adopter reads the shared feed");
    });

    assert!(
        feed.has_joiners(),
        "the creating start must see the adoption before deciding to stop"
    );
    assert_eq!(
        feed.wait_after(Some(0), WAIT).map(|frame| frame.frame_seq),
        Ok(1),
        "the creator still sees the frame the adopter read"
    );
}
