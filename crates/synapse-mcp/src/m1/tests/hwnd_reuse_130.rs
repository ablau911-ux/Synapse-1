//! Regression tests for HWND destroy/reuse in the detection tracker (#130).
//!
//! Windows recycles HWND values. A destroyed window and its successor can share
//! hwnd, pid, creator thread, and size, so only the capture crate's per-window
//! destroy generation can tell them apart. The tracker identity must carry that
//! generation so the successor never inherits the predecessor's tracks.

use super::*;

fn person(x: i32, y: i32) -> Detection {
    Detection {
        class_label: "person".to_owned(),
        bbox: Rect { x, y, w: 80, h: 40 },
        confidence: 0.9,
        track_id: None,
    }
}

fn identity(generation: u64) -> DetectionTargetIdentity {
    DetectionTargetIdentity {
        hwnd: 4242,
        pid: 10,
        thread_id: 1,
        width: 400,
        height: 300,
        generation,
    }
}

#[test]
fn must_fail_recycled_hwnd_with_identical_pid_thread_and_size_does_not_inherit_tracks() {
    let mut runtime = DetectionRuntime::default();
    let at = Utc::now();
    let origin = Rect {
        x: 0,
        y: 0,
        w: 400,
        h: 300,
    };

    let predecessor =
        runtime
            .tracker_for_target(identity(0))
            .update(vec![person(10, 20)], at, origin);
    // Same window again: the track must persist (guards against over-clearing).
    let same_window =
        runtime
            .tracker_for_target(identity(0))
            .update(vec![person(10, 20)], at, origin);
    assert_eq!(predecessor[0].track_id, same_window[0].track_id);

    // Destroyed and recreated: same hwnd value, only the generation advanced.
    let successor =
        runtime
            .tracker_for_target(identity(1))
            .update(vec![person(10, 20)], at, origin);
    assert_ne!(predecessor[0].track_id, successor[0].track_id);
    assert_eq!(runtime.tracker.active.len(), 1);
}

#[test]
fn must_pass_same_window_generation_keeps_its_track_across_frames() {
    let mut runtime = DetectionRuntime::default();
    let at = Utc::now();
    let origin = Rect {
        x: 0,
        y: 0,
        w: 400,
        h: 300,
    };

    let first = runtime
        .tracker_for_target(identity(3))
        .update(vec![person(10, 20)], at, origin);
    let second = runtime
        .tracker_for_target(identity(3))
        .update(vec![person(10, 20)], at, origin);
    let third = runtime
        .tracker_for_target(identity(3))
        .update(vec![person(10, 20)], at, origin);

    assert_eq!(first.len(), 1);
    // Track ids are allocated from 1; 0 would mean no track was assigned.
    assert_ne!(first[0].track_id, 0);
    assert_eq!(first[0].track_id, second[0].track_id);
    assert_eq!(second[0].track_id, third[0].track_id);
    assert_eq!(runtime.tracker.active.len(), 1);
}
