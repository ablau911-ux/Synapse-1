use super::*;

fn event(kind: InteractionEventKind, screen_point: Option<(i32, i32)>) -> InteractionEvent {
    InteractionEvent {
        ts_ns: 1,
        kind,
        injected: false,
        key_signal: None,
        screen_point,
    }
}

#[test]
fn must_pass_click_is_attributed_to_the_window_under_the_pointer() {
    assert_eq!(
        window_source(&event(InteractionEventKind::Click, Some((120, -40)))),
        WindowSource::TopLevelAtPoint { x: 120, y: -40 }
    );
}

#[test]
fn must_pass_wheel_scroll_is_attributed_to_the_window_under_the_pointer() {
    for kind in [
        InteractionEventKind::VerticalScroll { delta: 120 },
        InteractionEventKind::HorizontalScroll { delta: -120 },
    ] {
        assert_eq!(
            window_source(&event(kind, Some((5, 9)))),
            WindowSource::TopLevelAtPoint { x: 5, y: 9 }
        );
    }
}

#[test]
fn must_fail_keyboard_stays_on_the_foreground_window_even_with_a_stray_point() {
    assert_eq!(
        window_source(&event(InteractionEventKind::Keystroke, Some((1, 1)))),
        WindowSource::Foreground
    );
}

#[test]
fn must_fail_mouse_event_without_a_point_falls_back_to_foreground() {
    assert_eq!(
        window_source(&event(InteractionEventKind::Click, None)),
        WindowSource::Foreground
    );
}
