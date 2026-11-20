#[test]
fn must_pass_look_works_with_tiny_history_cap() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera_with(
        CameraConfig {
            history_max_bytes: 1,
            look_long_edge: 320,
            ..CameraConfig::default()
        },
        None,
    );
    let before = current(&camera, &window, FIRST);
    assert_eq!(camera.status().history_bytes, 0, "no record fits one byte");
    window.paint(SECOND);
    wait_for(|| color_matches(screen_pixel(window.rect()), SECOND).then_some(()));
    let after = current(&camera, &window, SECOND);
    assert!(after.seq > before.seq);
    assert_eq!(camera.status().history_bytes, 0);
    let zoom = camera
        .zoom(window.rect())
        .expect("zoom survives history eviction");
    let image = image::load_from_memory(&zoom.image_jpeg).unwrap().to_rgb8();
    assert!(color_matches(
        pixel(&image, image.width() as i32 / 2, image.height() as i32 / 2),
        SECOND
    ));
    assert!(camera.changes(before.seq, Some(0)).unwrap().since_evicted);
    // Even with no retained record, the current cursor is not an evicted cursor.
    // status() is cheap; look() encodes a JPEG, and a live desktop can advance the cursor
    // during that encode, so the pair would rarely match.
    let latest = wait_for(|| {
        let seq = camera.status().latest_seq;
        let changes = camera.changes(seq, Some(0)).unwrap();
        (changes.seq == seq).then_some(changes)
    });
    assert!(!latest.since_evicted);
    assert!(latest.regions.is_empty());
    window.assert_not_foreground();
}

#[test]
fn must_fail_concurrent_start_one_already_running() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let baseline = thread_ids();
    let camera = RunningCamera(Camera::default());
    let starts = concurrent_starts(&camera);
    assert_eq!(
        starts.iter().filter(|result| result.is_ok()).count(),
        1,
        "exactly one concurrent start succeeds: {starts:?}"
    );
    assert_eq!(
        starts
            .iter()
            .filter(|result| matches!(result, Err(synapse_livefeed::LivefeedError::AlreadyRunning)))
            .count(),
        1,
        "loser must receive LIVEFEED_ALREADY_RUNNING: {starts:?}"
    );
    current(&camera, &window, FIRST);
    let workers = camera_workers(&baseline);
    for name in ["synapse-livefeed", "synapse-capture"] {
        assert_eq!(
            workers.iter().filter(|(_, worker)| worker == name).count(),
            1,
            "concurrent start must create just one {name}: {workers:?}"
        );
    }
    camera
        .stop()
        .expect("join successful start and any rejected-start resources");
    assert!(!camera.status().running);
    assert!(
        camera_workers(&baseline).is_empty(),
        "camera worker survived stop"
    );
}

fn concurrent_starts(camera: &Camera) -> [synapse_livefeed::Result<()>; 2] {
    let gate = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        let start = || {
            gate.wait();
            camera.start(CameraConfig::default(), None)
        };
        let first = scope.spawn(start);
        let second = scope.spawn(start);
        gate.wait();
        [
            first.join().expect("first start joined"),
            second.join().expect("second start joined"),
        ]
    })
}

fn camera_workers(baseline: &std::collections::BTreeSet<u32>) -> Vec<(u32, String)> {
    thread_ids()
        .difference(baseline)
        .filter_map(|&id| {
            let name = thread_description(id)?;
            (name == "synapse-livefeed" || name.starts_with("synapse-capture"))
                .then_some((id, name))
        })
        .collect()
}

#[test]
fn must_pass_results_carry_monitor_and_origin() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let layout = monitors();
    let primary = *layout.iter().find(|monitor| monitor.primary).unwrap();
    check_monitor_results(&window, primary, None);
    // Also capture every explicit index, exercising negative/nonzero origins
    // on a multi-monitor PC without changing Windows display configuration.
    for monitor in layout {
        check_monitor_results(&window, monitor, Some(monitor.index));
    }
    window.move_to(64, 64);
    window.paint(FIRST);
}

fn check_monitor_results(window: &Window, monitor: Monitor, requested: Option<u32>) {
    assert!(
        monitor.rect.w >= 320 && monitor.rect.h >= 320,
        "monitor must fit the persistent 256px fixture plus 64px margin"
    );
    window.move_to(monitor.rect.x + 32, monitor.rect.y + 32);
    window.paint(FIRST);
    wait_for(|| color_matches(screen_pixel(window.rect()), FIRST).then_some(()));
    let camera = start_camera_with(
        CameraConfig {
            fps: 10,
            look_long_edge: 320,
            ..CameraConfig::default()
        },
        requested,
    );
    let before = current(&camera, window, FIRST);
    check_look_source(&camera, &before, monitor);
    window.paint(SECOND);
    let after = current(&camera, window, SECOND);
    let changes = camera.changes(before.seq, Some(0)).unwrap();
    assert_source(changes.monitor, changes.origin_x, changes.origin_y, monitor);
    assert!(!changes.since_evicted && !changes.too_many);
    assert!(changes.seq >= after.seq);
    check_virtual_regions(&changes, window.rect(), monitor.rect);
    check_zoom_source(&camera, window, monitor);
    camera
        .stop()
        .expect("stop before switching selected monitor");
    window.assert_not_foreground();
}

fn check_look_source(camera: &Camera, before: &Look, monitor: Monitor) {
    assert_source(before.monitor, before.origin_x, before.origin_y, monitor);
    assert_eq!(
        (before.width, before.height),
        (monitor.rect.w as u32, monitor.rect.h as u32)
    );
    let status = camera.status();
    assert_source(status.monitor, status.origin_x, status.origin_y, monitor);
    assert_eq!((status.width, status.height), (before.width, before.height));
}

fn check_zoom_source(camera: &Camera, window: &Window, monitor: Monitor) {
    let zoom = camera
        .zoom(window.rect())
        .expect("zoom takes virtual screen pixels");
    assert_source(zoom.monitor, zoom.origin_x, zoom.origin_y, monitor);
    assert_eq!(zoom.rect, window.rect());
    let image = image::load_from_memory(&zoom.image_jpeg).unwrap().to_rgb8();
    assert_eq!((image.width(), image.height()), (256, 256));
    assert!(color_matches(pixel(&image, 128, 128), SECOND));
    assert!(color_matches(screen_pixel(window.rect()), SECOND));
}

fn assert_source(index: u32, x: i32, y: i32, monitor: Monitor) {
    assert_eq!(index, monitor.index, "resolved Windows monitor index");
    assert_eq!(
        (x, y),
        (monitor.rect.x, monitor.rect.y),
        "physical monitor origin"
    );
}

fn check_virtual_regions(changes: &synapse_livefeed::Changes, window: Rect, screen: Rect) {
    for region in &changes.regions {
        assert!(region.rect.x >= screen.x && region.rect.y >= screen.y);
        assert!(region.rect.x + region.rect.w <= screen.x + screen.w);
        assert!(region.rect.y + region.rect.h <= screen.y + screen.h);
    }
    let (x, y) = (window.x + window.w / 2, window.y + window.h / 2);
    let region = changes
        .regions
        .iter()
        .find(|region| contains(region.rect, x, y))
        .expect("virtual changed rect covers the painted fixture center");
    let crop = image::load_from_memory(&region.crop_jpeg)
        .unwrap()
        .to_rgb8();
    assert_eq!(
        (crop.width(), crop.height()),
        (region.rect.w as u32, region.rect.h as u32)
    );
    assert!(color_matches(
        pixel(&crop, x - region.rect.x, y - region.rect.y),
        SECOND
    ));
}
