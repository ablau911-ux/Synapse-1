use super::*;

#[test]
fn must_fail_injected_input_never_refreshes_physical_activity() {
    let mut marker = LiveInputMarker::default();
    record_live_input(
        &mut marker,
        &InteractionEvent {
            ts_ns: 100,
            kind: InteractionEventKind::Keystroke,
            injected: false,
            key_signal: None,
            screen_point: None,
        },
        None,
    );
    record_live_input(
        &mut marker,
        &InteractionEvent {
            ts_ns: 200,
            kind: InteractionEventKind::Click,
            injected: true,
            key_signal: None,
            screen_point: None,
        },
        None,
    );
    assert_eq!(marker.last_physical_ns, 100);
    assert_eq!(marker.last_physical_kind, "keyboard");
    assert!(marker.user_window.is_none());
}

#[test]
fn must_pass_physical_click_and_scroll_update_event_class_only() {
    let mut marker = LiveInputMarker::default();
    for (ts_ns, kind, expected) in [
        (100, InteractionEventKind::Click, "click"),
        (
            200,
            InteractionEventKind::VerticalScroll { delta: 120 },
            "scroll",
        ),
    ] {
        record_live_input(
            &mut marker,
            &InteractionEvent {
                ts_ns,
                kind,
                injected: false,
                key_signal: None,
                screen_point: None,
            },
            None,
        );
        assert_eq!(marker.last_physical_ns, ts_ns);
        assert_eq!(marker.last_physical_kind, expected);
    }
}

#[test]
fn must_fail_injected_input_does_not_replace_the_human_window() {
    let Ok(window) = serde_json::from_value::<ForegroundContext>(serde_json::json!({
        "hwnd": 42, "pid": 7, "process_name": "editor.exe",
        "process_path": "C:\\editor.exe", "window_title": "Human work",
        "window_bounds": {"x": 0, "y": 0, "w": 800, "h": 600},
        "monitor_index": 0, "dpi_scale": 1.0,
        "is_fullscreen": false, "is_dwm_composed": true
    })) else {
        panic!("invalid synthetic window fixture");
    };
    let mut marker = LiveInputMarker::default();
    let physical = InteractionEvent {
        ts_ns: 100,
        kind: InteractionEventKind::Keystroke,
        injected: false,
        key_signal: None,
        screen_point: None,
    };
    record_live_input(&mut marker, &physical, Some(window));
    record_live_input(
        &mut marker,
        &InteractionEvent {
            ts_ns: 200,
            injected: true,
            ..physical
        },
        None,
    );
    assert_eq!(
        marker.user_window.as_ref().map(|window| window.hwnd),
        Some(42)
    );
    assert_eq!(marker.last_physical_ns, 100);
}
