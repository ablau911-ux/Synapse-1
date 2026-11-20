#![allow(clippy::unwrap_used, clippy::expect_used)]

use synapse_core::Rect;
use synapse_livefeed::tiles::diff;

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

fn change(frame: &mut [u8], stride: usize, x: usize, y: usize, channel: usize) {
    frame[y * stride + x * 4 + channel] ^= 0xff;
}

#[test]
fn must_pass_same_frame_no_rects() {
    let stride = 18 * 4 + 12;
    let previous = vec![73; stride * 19];
    assert!(diff(&previous, &previous, 18, 19, stride, 8).is_empty());
    let mut next = previous.clone();
    for row in next.chunks_mut(stride) {
        row[18 * 4..].fill(0);
    }
    assert!(
        diff(&previous, &next, 18, 19, stride, 8).is_empty(),
        "row padding is not a screen pixel"
    );
}

#[test]
fn must_pass_one_pixel_one_tile() {
    let stride = 18 * 4 + 12;
    let previous = vec![0; stride * 19];
    for channel in 0..4 {
        let mut next = previous.clone();
        change(&mut next, stride, 11, 3, channel);
        assert_eq!(
            diff(&previous, &next, 18, 19, stride, 8),
            vec![rect(8, 0, 8, 8)],
            "every BGRA channel counts: channel {channel}"
        );
        let mut edge = previous.clone();
        change(&mut edge, stride, 17, 18, channel);
        assert_eq!(
            diff(&previous, &edge, 18, 19, stride, 8),
            vec![rect(16, 16, 2, 3)],
            "edge tile must stop at the frame boundary"
        );
    }
}

#[test]
fn must_pass_adjacent_tiles_merge() {
    let previous = vec![0; 24 * 16 * 4];
    let mut next = previous.clone();
    for (x, y) in [(1, 1), (9, 1), (9, 9)] {
        change(&mut next, 24 * 4, x, y, 0);
    }
    assert_eq!(
        diff(&previous, &next, 24, 16, 24 * 4, 8),
        vec![rect(0, 0, 16, 16)],
        "4-connected L shape merges transitively into its bounding rectangle"
    );
}

#[test]
fn must_pass_diagonal_tiles_separate() {
    let previous = vec![0; 24 * 24 * 4];
    let mut next = previous.clone();
    for (x, y) in [(17, 17), (9, 9), (1, 1)] {
        change(&mut next, 24 * 4, x, y, 2);
    }
    assert_eq!(
        diff(&previous, &next, 24, 24, 24 * 4, 8),
        vec![rect(0, 0, 8, 8), rect(8, 8, 8, 8), rect(16, 16, 8, 8)],
        "diagonal contact is not adjacency; regions must be top-left sorted"
    );
}

#[test]
fn must_fail_size_mismatch_whole_frame() {
    let full = vec![0; 18 * 19 * 4];
    for (previous, next) in [
        (&full[..full.len() - 1], full.as_slice()),
        (full.as_slice(), &full[..full.len() - 1]),
        (&[][..], full.as_slice()),
    ] {
        assert_eq!(
            diff(previous, next, 18, 19, 18 * 4, 8),
            vec![rect(0, 0, 18, 19)],
            "a size mismatch invalidates the whole frame"
        );
    }
}
