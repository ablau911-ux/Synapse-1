//! Regression tests for #196: the recording backend keeps a bounded event
//! window, hands each action exactly its own events, and never labels typed
//! text for logging.

use synapse_core::{Action, Backend, Key, KeyCode, Point};

use super::{MAX_RECORDED_EVENTS, RecordedInput, RecordingBackend, redacted_key_label};
use crate::EmitState;

fn scroll(dy: i32) -> Action {
    Action::MouseScroll {
        dy,
        dx: 0,
        at: Some(Point { x: 1, y: 2 }),
        backend: Backend::Software,
    }
}

fn key(code: KeyCode) -> Key {
    Key {
        code,
        use_scancode: false,
    }
}

#[test]
fn must_fail_event_window_is_bounded_but_the_total_keeps_counting() {
    let backend = RecordingBackend::new();
    let mut state = EmitState::new();
    let total = MAX_RECORDED_EVENTS + 25;
    for _ in 0..total {
        backend
            .execute_capturing(&scroll(1), &mut state)
            .expect("scroll");
    }
    assert_eq!(backend.event_count(), total);
    assert_eq!(backend.events().len(), MAX_RECORDED_EVENTS);
    // A cursor taken before the eviction still resolves to the retained tail.
    assert_eq!(backend.events_since(total - 3).len(), 3);
    assert_eq!(backend.events_since(0).len(), MAX_RECORDED_EVENTS);
}

#[test]
fn must_pass_execute_capturing_returns_only_this_calls_events() {
    let backend = RecordingBackend::new();
    let mut state = EmitState::new();
    let first = backend
        .execute_capturing(&scroll(1), &mut state)
        .expect("first");
    let second = backend
        .execute_capturing(&scroll(-7), &mut state)
        .expect("second");
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert!(matches!(
        second[0],
        RecordedInput::MouseScroll { dy: -7, .. }
    ));
    assert_eq!(backend.event_count(), 2);
}

#[test]
fn must_fail_log_labels_never_carry_typed_text() {
    let unit = RecordedInput::UnicodeUnitDown { unit: 0x0041 };
    assert_eq!(unit.log_label(), "unicode_unit_down");
    assert_eq!(
        RecordedInput::UnicodeUnitUp { unit: 0x0041 }.log_label(),
        "unicode_unit_up"
    );
    let typed = RecordedInput::KeyDown {
        key: key(KeyCode::Symbol { value: 'p' }),
    };
    assert_eq!(typed.log_label(), "key_down:char");
    assert_eq!(
        redacted_key_label(&key(KeyCode::HidCode { value: 4 })),
        "hid"
    );
    assert_eq!(
        redacted_key_label(&key(KeyCode::Named {
            value: "a".to_owned()
        })),
        "char"
    );
    assert_eq!(
        redacted_key_label(&key(KeyCode::Named {
            value: "ctrl".to_owned()
        })),
        "ctrl"
    );
}
