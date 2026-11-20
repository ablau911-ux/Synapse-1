#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::Camera;
use crate::history::RawBgraFrame;
use crate::history::{ChangeRecord, History};
use synapse_core::Rect;

#[test]
fn must_fail_size_change_is_whole_frame_change() {
    // Both shapes have eight pixels: byte-length comparison cannot detect this
    // resize. The second case also returns to the cursor's original dimensions.
    for original in [(4, 2), (2, 4)] {
        let camera = resized_history_camera(original);
        let changes = camera
            .changes(1, Some(0))
            .expect("changes across size change");
        assert_eq!((changes.since, changes.seq), (1, 3));
        assert!(!changes.since_evicted && !changes.too_many);
        assert_eq!(
            changes.regions.len(),
            1,
            "resize invalidates the entire current frame"
        );
        let region = &changes.regions[0];
        assert_eq!(
            region.rect,
            Rect {
                x: changes.origin_x,
                y: changes.origin_y,
                w: 2,
                h: 4
            }
        );
        let image = image::load_from_memory(&region.crop_jpeg)
            .unwrap()
            .to_rgb8();
        assert_eq!((image.width(), image.height()), (2, 4));
        for color in image.pixels() {
            assert!(
                color
                    .0
                    .into_iter()
                    .zip([90_u8, 200, 40])
                    .all(|(actual, expected)| actual.abs_diff(expected) <= 20),
                "crop must contain the current frame's pixels"
            );
        }
    }
}

fn resized_history_camera(original: (u32, u32)) -> Camera {
    let camera = Camera::default();
    {
        let mut state = camera.lock_state();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        state.running = true;
        state.cfg.history_ms = u64::MAX;
        state.width = 2;
        state.height = 4;
        state.latest_seq = 3;
        state.latest_ts_ms = now;
        state.history = History::new(u64::MAX, 1_024);
        populate_history(&mut state.history, original, now);
        state.latest_raw = Some(RawBgraFrame {
            width: 2,
            height: 4,
            row_stride: 8,
            bgra: [40, 200, 90, 255].repeat(8),
        });
    }
    camera
}

fn populate_history(history: &mut History, original: (u32, u32), now: u64) {
    for (seq, (width, height)) in [(1, original), (2, (4, 2)), (3, (2, 4))] {
        history.push(ChangeRecord {
            seq,
            ts_ms: now,
            width,
            height,
            rects: vec![Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            }],
        });
    }
}

#[test]
fn must_pass_many_scattered_changes_merge_fast() {
    let expected = scattered_tiles();
    let camera = scattered_history_camera(&expected);
    let started = std::time::Instant::now();
    // Exercise the actual history union through the public camera API, not a
    // second merge implementation. Setup and exact-result checks are untimed.
    let changes = camera.changes(1, Some(0)).unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "4,096 scattered history rects must merge under 100ms: {elapsed:?}"
    );
    assert_eq!((changes.since, changes.seq), (1, 129));
    assert!(!changes.since_evicted);
    assert!(changes.too_many, "disconnected cells exceed the crop cap");
    assert_eq!(
        changes.regions.iter().map(|r| r.rect).collect::<Vec<_>>(),
        expected,
        "deduplicate repeated cells; diagonal contact must stay disconnected"
    );
    assert!(changes.regions.iter().all(|r| r.crop_jpeg.is_empty()));
}

fn scattered_tiles() -> Vec<Rect> {
    // 288 isolated checkerboard cells spread over a 32x18 tile grid.
    (0..18)
        .flat_map(|row| {
            (0..32).filter_map(move |col| {
                ((row + col) % 2 == 0).then_some(Rect {
                    x: col * 64,
                    y: row * 64,
                    w: 64,
                    h: 64,
                })
            })
        })
        .collect()
}

fn scattered_history_camera(rects: &[Rect]) -> Camera {
    let camera = Camera::default();
    {
        let mut state = camera.lock_state();
        let now = crate::history::current_unix_ms();
        state.running = true;
        state.cfg.history_ms = u64::MAX;
        state.cfg.tile_px = 64;
        state.cfg.max_change_crops = 8;
        state.width = 2_048;
        state.height = 1_152;
        state.latest_seq = 129;
        state.latest_ts_ms = now;
        state.history = History::new(u64::MAX, 1_048_576);
        state
            .history
            .push(ChangeRecord::new(1, now, 2_048, 1_152, vec![]));
        for record in 0..128 {
            let changed = (0..32)
                .map(|cell| rects[(record * 32 + cell) % rects.len()])
                .collect();
            state.history.push(ChangeRecord::new(
                u64::try_from(record).unwrap() + 2,
                now,
                2_048,
                1_152,
                changed,
            ));
        }
        state.latest_raw = Some(RawBgraFrame {
            width: 2_048,
            height: 1_152,
            row_stride: 2_048 * 4,
            bgra: [40, 200, 90, 255].repeat(2_048 * 1_152),
        });
    }
    camera
}
