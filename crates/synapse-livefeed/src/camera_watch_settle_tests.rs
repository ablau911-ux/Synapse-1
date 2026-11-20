#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Deterministic tests for the cut flag, settle waits and the watch grid layout. A fake
//! running camera (no capture thread) is driven by pushing frames into its state directly.

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use super::{Camera, watch::grid_layout};
use crate::history::RawBgraFrame;
use crate::history::{ChangeRecord, History, current_unix_ms};
use synapse_core::Rect;

const W: u32 = 100;
const H: u32 = 100;

fn fake_camera() -> Arc<Camera> {
    let camera = Arc::new(Camera::default());
    {
        let mut s = camera.lock_state();
        s.running = true;
        s.cfg.history_ms = u64::MAX;
        s.cfg.tile_px = 10;
        s.width = W;
        s.height = H;
        s.latest_seq = 1;
        s.latest_ts_ms = current_unix_ms();
        s.history = History::new(u64::MAX, 1_048_576);
        s.history
            .push(ChangeRecord::new(1, current_unix_ms(), W, H, vec![]));
        s.latest_raw = Some(RawBgraFrame {
            width: W,
            height: H,
            row_stride: (W * 4) as usize,
            bgra: [40, 200, 90, 255].repeat((W * H) as usize),
        });
    }
    camera
}

/// Simulates the consumer storing one changed frame and waking waiters.
fn store_frame(camera: &Camera, rect: Rect) {
    let mut s = camera.lock_state();
    let seq = s.latest_seq + 1;
    s.latest_seq = seq;
    s.latest_ts_ms = current_unix_ms();
    s.history
        .push(ChangeRecord::new(seq, current_unix_ms(), W, H, vec![rect]));
    drop(s);
    camera.shared.condvar.notify_all();
}

const fn rect(w: i32, h: i32) -> Rect {
    Rect { x: 0, y: 0, w, h }
}

#[test]
fn must_pass_cut_flag_when_changed_area_reaches_ratio() {
    let camera = fake_camera();
    store_frame(&camera, rect(50, 50)); // 25% of the frame
    camera.lock_state().cfg.cut_ratio_percent = 20;
    let at_20 = camera.changes(1, Some(0)).unwrap();
    assert_eq!(at_20.regions.len(), 1);
    assert!(
        at_20.cut,
        "25% changed area must be a cut at a 20% threshold"
    );
    camera.lock_state().cfg.cut_ratio_percent = 25;
    assert!(
        camera.changes(1, Some(0)).unwrap().cut,
        "the threshold is inclusive"
    );
}

#[test]
fn must_fail_small_change_is_not_a_cut() {
    let camera = fake_camera();
    store_frame(&camera, rect(10, 10)); // 1% of the frame
    camera.lock_state().cfg.cut_ratio_percent = 20;
    let changes = camera.changes(1, Some(0)).unwrap();
    assert_eq!(
        changes.regions.len(),
        1,
        "the change itself is still reported"
    );
    assert!(!changes.cut, "a 1% change must not ask the agent to re-key");
    camera.lock_state().cfg.cut_ratio_percent = 26;
    store_frame(&camera, rect(10, 10));
    assert!(!camera.changes(1, Some(0)).unwrap().cut);
}

/// Pushes a frame every `every` for `burst`, then goes quiet.
fn burst_then_quiet(
    camera: &Arc<Camera>,
    every: Duration,
    burst: Duration,
) -> thread::JoinHandle<()> {
    let camera = Arc::clone(camera);
    thread::spawn(move || {
        let end = Instant::now() + burst;
        while Instant::now() < end {
            store_frame(&camera, rect(10, 10));
            thread::sleep(every);
        }
    })
}

#[test]
fn must_pass_settle_returns_once_quiet_for_the_window() {
    let camera = fake_camera();
    let started = Instant::now();
    let pusher = burst_then_quiet(
        &camera,
        Duration::from_millis(40),
        Duration::from_millis(400),
    );
    let changes = camera.changes_with(1, Some(5_000), Some(300)).unwrap();
    let elapsed = started.elapsed();
    pusher.join().unwrap();
    assert!(
        changes.settled,
        "screen went quiet, so the wait must report settled"
    );
    assert!(
        elapsed >= Duration::from_millis(650),
        "must not return during the 400ms burst or before 300ms of quiet: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(2_000),
        "must return soon after the quiet window, not at the 5s budget: {elapsed:?}"
    );
    assert!(changes.seq > 1, "the burst's frames are reported");
    assert!(!changes.regions.is_empty());
}

#[test]
fn must_fail_settle_reports_unsettled_when_screen_never_quiet() {
    let camera = fake_camera();
    let started = Instant::now();
    let pusher = burst_then_quiet(
        &camera,
        Duration::from_millis(40),
        Duration::from_millis(1_500),
    );
    let changes = camera.changes_with(1, Some(700), Some(300)).unwrap();
    let elapsed = started.elapsed();
    pusher.join().unwrap();
    assert!(
        !changes.settled,
        "a busy screen must never be reported as settled"
    );
    assert!(
        elapsed >= Duration::from_millis(650) && elapsed < Duration::from_millis(1_400),
        "must give up at the 700ms budget: {elapsed:?}"
    );
}

#[test]
fn must_pass_settle_already_quiet_returns_after_one_window() {
    let camera = fake_camera();
    let started = Instant::now();
    let changes = camera.changes_with(1, Some(5_000), Some(200)).unwrap();
    let elapsed = started.elapsed();
    assert!(changes.settled);
    assert!(
        elapsed >= Duration::from_millis(190) && elapsed < Duration::from_millis(1_000),
        "{elapsed:?}"
    );
}

#[test]
fn must_fail_watch_static_screen_gives_one_cell() {
    let camera = fake_camera();
    let watch = camera.watch(Some(400), Some(5)).unwrap();
    assert_eq!(
        watch.frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
        vec![1],
        "an unchanged screen must not be sampled twice"
    );
    assert_eq!((watch.cols, watch.rows), (1, 1));
    assert!(
        watch.ms_elapsed >= 380,
        "watch must still cover the requested span"
    );
}

#[test]
fn must_pass_watch_adds_cells_only_for_new_frames() {
    let camera = fake_camera();
    let pusher = burst_then_quiet(
        &camera,
        Duration::from_millis(30),
        Duration::from_millis(1_000),
    );
    let watch = camera.watch(Some(800), Some(4)).unwrap();
    pusher.join().unwrap();
    let seqs: Vec<u64> = watch.frames.iter().map(|f| f.seq).collect();
    assert_eq!(
        seqs.len(),
        4,
        "a moving screen fills every cadence slot: {seqs:?}"
    );
    assert!(
        seqs.windows(2).all(|p| p[1] > p[0]),
        "strictly newer frames: {seqs:?}"
    );
    let image = image::load_from_memory(&watch.image_jpeg).unwrap();
    assert_eq!(
        (image.width(), image.height()),
        (2 * W + 2, 2 * H + 2),
        "2x2 grid of 100x100 cells with one 2px separator each way"
    );
}

/// Paints `rect` (frame px) of the fake camera's current frame red (BGRA).
fn paint_red(camera: &Camera, rect: Rect) {
    let mut s = camera.lock_state();
    let raw = s.latest_raw.as_mut().unwrap();
    for y in rect.y..rect.y + rect.h {
        for x in rect.x..rect.x + rect.w {
            let i = (y as usize * W as usize + x as usize) * 4;
            raw.bgra[i..i + 4].copy_from_slice(&[0, 0, 255, 255]);
        }
    }
}

#[test]
fn must_pass_watch_region_films_only_that_area_at_native_size() {
    let camera = fake_camera();
    let region = Rect {
        x: 20,
        y: 30,
        w: 40,
        h: 20,
    };
    paint_red(&camera, region);
    camera.lock_state().cfg.watch_cell_long_edge = 480;
    let watch = camera
        .watch_region(Some(200), Some(1), Some(region))
        .unwrap();
    assert_eq!(watch.region, Some(region));
    assert_eq!(
        (watch.cell_w, watch.cell_h),
        (40, 20),
        "smaller than a cell: native pixels"
    );
    let image = image::load_from_memory(&watch.image_jpeg)
        .unwrap()
        .to_rgb8();
    assert_eq!((image.width(), image.height()), (40, 20));
    let [r, g, b] = image.get_pixel(20, 10).0;
    assert!(
        r > 200 && g < 60 && b < 60,
        "cell must show the region, not the frame: {r},{g},{b}"
    );
}

#[test]
fn must_pass_watch_region_larger_than_cell_is_shrunk() {
    let camera = fake_camera();
    camera.lock_state().cfg.watch_cell_long_edge = 50;
    let region = Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 50,
    };
    let watch = camera
        .watch_region(Some(200), Some(1), Some(region))
        .unwrap();
    assert_eq!(
        (watch.cell_w, watch.cell_h),
        (50, 25),
        "long edge capped, aspect kept"
    );
}

#[test]
fn must_fail_watch_region_outside_frame_is_bad_region() {
    let camera = fake_camera();
    for region in [
        Rect {
            x: 90,
            y: 90,
            w: 20,
            h: 20,
        },
        Rect {
            x: -1,
            y: 0,
            w: 10,
            h: 10,
        },
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 10,
        },
    ] {
        let err = camera
            .watch_region(Some(200), Some(1), Some(region))
            .unwrap_err();
        assert!(
            matches!(err, crate::error::LivefeedError::BadRegion { .. }),
            "{region:?} -> {err:?}"
        );
    }
}

#[test]
fn must_pass_watch_grid_layout_is_square_ish_row_major() {
    let expected = [
        (0, (0, 0)),
        (1, (1, 1)),
        (2, (2, 1)),
        (3, (2, 2)),
        (4, (2, 2)),
        (5, (3, 2)),
        (6, (3, 2)),
        (7, (3, 3)),
        (8, (3, 3)),
        (9, (3, 3)),
        (10, (4, 3)),
    ];
    for (n, layout) in expected {
        assert_eq!(grid_layout(n), layout, "n={n}");
        let (cols, rows) = layout;
        assert!(cols * rows >= n, "every frame needs a cell (n={n})");
    }
}
