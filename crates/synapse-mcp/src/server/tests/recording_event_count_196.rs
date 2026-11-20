//! #196: the act_type foreground-lost readback counts recorded events in O(1)
//! and keeps counting past the retained-window cap.

use std::sync::Arc;

use synapse_action::{EmitState, MAX_RECORDED_EVENTS, RecordingBackend};
use synapse_core::{Action, Backend, Point};

use super::recording_event_count;

fn scroll() -> Action {
    Action::MouseScroll {
        dy: 1,
        dx: 0,
        at: Some(Point { x: 1, y: 2 }),
        backend: Backend::Software,
    }
}

#[test]
fn must_pass_event_count_keeps_counting_past_the_retained_window() {
    let backend = Arc::new(RecordingBackend::new());
    let mut state = EmitState::new();
    let total = MAX_RECORDED_EVENTS + 25;
    for _ in 0..total {
        assert!(backend.execute_capturing(&scroll(), &mut state).is_ok());
    }
    assert_eq!(recording_event_count(Some(&backend)), total);
}

#[test]
fn must_fail_no_recording_backend_counts_zero_events() {
    assert_eq!(recording_event_count(None), 0);
}
