#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Locked-screen rules, proven on a fake camera (no capture thread, the owner's session is
//! never locked): no operation returns an image while locked, waiters wake on the lock, and
//! unlocking a static desktop makes the kept frame current again.

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use super::{Camera, ScreenState, lifecycle::apply_lock_state};
use crate::history::{ChangeRecord, History, RawBgraFrame, current_unix_ms};
use synapse_core::Rect;

fn fake_camera() -> Arc<Camera> {
    let camera = Arc::new(Camera::default());
    {
        let mut s = camera.lock_state();
        s.running = true;
        s.cfg.history_ms = u64::MAX;
        s.width = 64;
        s.height = 64;
        s.latest_seq = 2;
        s.latest_ts_ms = current_unix_ms();
        s.history = History::new(u64::MAX, 1_048_576);
        s.history
            .push(ChangeRecord::new(1, current_unix_ms(), 64, 64, vec![]));
        s.history.push(ChangeRecord::new(
            2,
            current_unix_ms(),
            64,
            64,
            vec![Rect {
                x: 0,
                y: 0,
                w: 32,
                h: 32,
            }],
        ));
        s.latest_raw = Some(RawBgraFrame {
            width: 64,
            height: 64,
            row_stride: 256,
            bgra: [40, 200, 90, 255].repeat(64 * 64),
        });
    }
    camera
}

fn lock(camera: &Camera, locked: bool) -> bool {
    let changed = apply_lock_state(&mut camera.lock_state(), locked);
    camera.shared.condvar.notify_all();
    changed
}

const REGION: Rect = Rect {
    x: 0,
    y: 0,
    w: 16,
    h: 16,
};

#[test]
fn must_fail_locked_screen_returns_no_image_from_any_operation() {
    let camera = fake_camera();
    assert!(lock(&camera, true), "Live -> Locked is a transition");
    let look = camera.look().unwrap();
    assert_eq!(look.screen_state, ScreenState::Locked);
    assert!(
        look.image_jpeg.is_empty(),
        "look must not show the pre-lock frame"
    );
    let zoom = camera.zoom(REGION).unwrap();
    assert_eq!(zoom.screen_state, ScreenState::Locked);
    assert!(
        zoom.image_jpeg.is_empty(),
        "zoom must not show the pre-lock frame"
    );
    let changes = camera.changes(1, Some(0)).unwrap();
    assert_eq!(changes.screen_state, ScreenState::Locked);
    assert!(changes.regions.is_empty(), "no crops while locked");
    let watch = camera.watch(Some(100), Some(2)).unwrap();
    assert_eq!(watch.screen_state, ScreenState::Locked);
    assert!(watch.image_jpeg.is_empty() && watch.frames.is_empty());
    assert_eq!(camera.status().screen_state, ScreenState::Locked);
}

#[test]
fn must_pass_lock_wakes_a_waiting_changes_call() {
    let camera = fake_camera();
    let waiter = {
        let camera = Arc::clone(&camera);
        thread::spawn(move || {
            let started = Instant::now();
            let changes = camera.changes(2, Some(5_000)).unwrap();
            (changes.screen_state, started.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(150));
    lock(&camera, true);
    let (state, elapsed) = waiter.join().unwrap();
    assert_eq!(
        state,
        ScreenState::Locked,
        "the waiter learns the screen locked"
    );
    assert!(
        elapsed < Duration::from_millis(1_500),
        "a lock must wake the waiter, not let it sit out its 5s budget: {elapsed:?}"
    );
}

#[test]
fn must_pass_unlock_on_static_desktop_makes_kept_frame_current() {
    let camera = fake_camera();
    lock(&camera, true);
    assert!(!lock(&camera, true), "re-locking is not a transition");
    assert!(lock(&camera, false), "Locked -> Live is a transition");
    let look = camera.look().unwrap();
    assert_eq!(look.screen_state, ScreenState::Live);
    assert!(
        !look.image_jpeg.is_empty(),
        "the kept frame is shown again after unlock"
    );
    assert_eq!(look.seq, 2, "unlock does not invent a new frame");
    assert!(
        !lock(&camera, false),
        "unlocking a live screen is not a transition"
    );
}

#[test]
fn must_fail_unlock_without_any_frame_stays_locked() {
    let camera = fake_camera();
    lock(&camera, true);
    camera.lock_state().latest_raw = None;
    assert!(
        !lock(&camera, false),
        "no frame to show: stay locked until capture sends one"
    );
    assert_eq!(camera.status().screen_state, ScreenState::Locked);
}
