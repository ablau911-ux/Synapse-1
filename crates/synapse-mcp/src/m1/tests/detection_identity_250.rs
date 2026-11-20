//! Child of detection.rs. Main already fixed; supplements existing generation
//! reuse tests with distinct HWND/PID and geometry edge coverage.
use super::*;

fn identity(hwnd: i64, pid: u32) -> DetectionTargetIdentity {
    DetectionTargetIdentity {
        hwnd,
        pid,
        thread_id: 1,
        width: 400,
        height: 300,
        generation: 0,
    }
}

fn detection() -> Detection {
    Detection {
        class_label: "person".into(),
        bbox: Rect {
            x: 10,
            y: 20,
            w: 80,
            h: 40,
        },
        confidence: 0.9,
        track_id: None,
    }
}

#[test]
fn must_pass_stable_target_keeps_track_and_exact_capture_origin() {
    let mut runtime = DetectionRuntime::default();
    let at = Utc::now();
    let bounds = Rect {
        x: -400,
        y: 25,
        w: 400,
        h: 300,
    };
    assert_eq!(
        stable_capture_origin(bounds, bounds, 400, 300),
        Some(bounds)
    );
    let first = runtime
        .tracker_for_target(identity(42, 10))
        .update(vec![detection()], at, bounds);
    assert_eq!(first.len(), 1);
    let next = runtime
        .tracker_for_target(identity(42, 10))
        .update(vec![detection()], at, bounds);
    assert_eq!(next[0].track_id, first[0].track_id);
}

#[test]
fn must_fail_changed_hwnd_or_pid_cannot_inherit_previous_tracks() {
    for changed in [identity(43, 10), identity(42, 11)] {
        let mut runtime = DetectionRuntime::default();
        let at = Utc::now();
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 400,
            h: 300,
        };
        let first =
            runtime
                .tracker_for_target(identity(42, 10))
                .update(vec![detection()], at, bounds);
        assert_eq!(runtime.tracker.active.len(), 1);
        let next = runtime
            .tracker_for_target(changed)
            .update(vec![detection()], at, bounds);
        assert_ne!(next[0].track_id, first[0].track_id);
        assert_eq!(runtime.tracker.active.len(), 1);
    }
}

#[test]
fn must_fail_geometry_drift_or_bitmap_extent_mismatch_has_no_origin() {
    let before = Rect {
        x: 10,
        y: 20,
        w: 400,
        h: 300,
    };
    assert_eq!(
        stable_capture_origin(before, before, 400, 300),
        Some(before)
    );
    for after in [
        Rect { x: 11, ..before },
        Rect { y: 21, ..before },
        Rect { w: 401, ..before },
        Rect { h: 301, ..before },
    ] {
        assert_eq!(stable_capture_origin(before, after, 400, 300), None);
    }
    for (w, h) in [(0, 300), (400, 0), (399, 300), (400, 299), (u32::MAX, 300)] {
        assert_eq!(stable_capture_origin(before, before, w, h), None);
    }
}
