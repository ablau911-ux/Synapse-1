#[test]
fn must_fail_cursor_from_previous_run_is_evicted() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    current(&camera, &window, FIRST);
    window.paint(SECOND);
    let old_cursor = current(&camera, &window, SECOND).seq;
    camera.stop().expect("stop first run on the same Camera");
    let old_latest = camera.status().latest_seq;
    assert!(old_latest >= old_cursor);

    camera
        .start(
            CameraConfig {
                fps: 10,
                look_long_edge: 320,
                ..CameraConfig::default()
            },
            None,
        )
        .expect("restart the same Camera with the persistent fixture");
    let restarted = current(&camera, &window, SECOND);
    assert!(
        restarted.seq > old_latest,
        "new run must start above the previous run's latest sequence"
    );
    let changes = camera.changes(old_cursor, Some(0)).unwrap();
    assert_eq!(changes.since, old_cursor);
    assert!(
        changes.since_evicted,
        "old-run cursor is not a new baseline"
    );
    assert!(changes.seq >= restarted.seq);
    assert!(changes.regions.is_empty(), "evicted cursor requires look");

    window.paint(FIRST);
    let repainted = current(&camera, &window, FIRST);
    assert!(repainted.seq > restarted.seq);
    assert!(repainted.seq > old_latest);
    assert!(camera.changes(old_cursor, Some(0)).unwrap().since_evicted);
    assert!(color_matches(screen_pixel(window.rect()), FIRST));
    window.assert_not_foreground();
}

#[test]
fn must_fail_cursor_beyond_latest_is_evicted_without_waiting() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    current(&camera, &window, FIRST);
    let since = camera.status().latest_seq.checked_add(100).unwrap();
    let started = Instant::now();
    let changes = camera.changes(since, Some(5_000)).unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "invalid future cursor consumed the wait budget: {elapsed:?}"
    );
    assert_eq!(changes.since, since);
    assert!(changes.since_evicted);
    assert!(changes.seq < since);
    assert!(changes.regions.is_empty());
    window.assert_not_foreground();
}

#[test]
fn must_pass_wait_is_capped_by_wait_max_ms() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let wait_max_ms = 75;
    let margin = Duration::from_secs(1);
    let camera = start_camera_with(
        CameraConfig {
            fps: 10,
            look_long_edge: 320,
            wait_max_ms,
            ..CameraConfig::default()
        },
        None,
    );
    current(&camera, &window, FIRST);
    let cursor = camera.status().latest_seq;
    let started = Instant::now();
    camera.changes(cursor, Some(60_000)).unwrap();
    let elapsed = started.elapsed();
    // Other desktop changes may wake the call early. No lower-bound, empty-
    // regions, or unchanged-sequence assertion is valid on a live desktop.
    assert!(
        elapsed <= Duration::from_millis(wait_max_ms) + margin,
        "requested wait must be capped by config: {elapsed:?}"
    );
    assert!(color_matches(screen_pixel(window.rect()), FIRST));
    window.assert_not_foreground();
}
