use synapse_livefeed::camera::Watch;

const WATCH_SEPARATOR: u32 = 2;

fn watch_window_colors(watch: &Watch, before: &Look, rect: Rect) -> Vec<[u8; 3]> {
    let strip = image::load_from_memory(&watch.image_jpeg)
        .expect("watch must return a decodable strip JPEG")
        .to_rgb8();
    assert_eq!(watch.monitor, before.monitor);
    assert_eq!(
        (watch.origin_x, watch.origin_y),
        (before.origin_x, before.origin_y)
    );
    assert_eq!(format!("{:?}", watch.screen_state), "Live");
    assert!(watch.cols > 0 && watch.rows > 0);
    let scale = f64::from(watch.cell_w) / f64::from(before.width);
    let x = (f64::from(rect.x + rect.w / 2 - watch.origin_x) * scale) as u32;
    let y = (f64::from(rect.y + rect.h / 2 - watch.origin_y) * scale) as u32;
    assert!(
        x < watch.cell_w && y < watch.cell_h,
        "window center must be inside each cell"
    );
    watch
        .frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            assert_eq!(frame.index, index);
            let index = u32::try_from(index).unwrap();
            let cols = u32::try_from(watch.cols).unwrap();
            let cell_x = (index % cols) * (watch.cell_w + WATCH_SEPARATOR);
            let cell_y = (index / cols) * (watch.cell_h + WATCH_SEPARATOR);
            // Virtual window -> frame pixels -> scaled cell pixels -> strip pixels.
            strip.get_pixel(cell_x + x, cell_y + y).0
        })
        .collect()
}

fn assert_watch_frame_order(watch: &Watch) {
    assert!(!watch.frames.is_empty(), "watch must sample immediately");
    for (index, frame) in watch.frames.iter().enumerate() {
        assert_eq!(frame.index, index);
    }
    for pair in watch.frames.windows(2) {
        assert!(
            pair[1].seq > pair[0].seq,
            "unchanged sequences must not be sampled twice"
        );
        assert!(
            pair[1].ts_ms >= pair[0].ts_ms,
            "strip must preserve time order"
        );
    }
}

// Channel closure stops the painter even when an assertion unwinds the scope.
fn with_watch_paints(window: &Window, run: impl FnOnce()) {
    std::thread::scope(|scope| {
        let (stop, receiver) = std::sync::mpsc::channel::<()>();
        let painter = scope.spawn(move || {
            let mut color = SECOND;
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                receiver.recv_timeout(Duration::from_millis(70))
            {
                window.paint(color);
                color = if color == SECOND { FIRST } else { SECOND };
            }
        });
        run();
        drop(stop);
        painter.join().expect("join synthetic painter");
    });
}

fn record_watch_motion(camera: &Camera, window: &Window) -> Watch {
    std::thread::scope(|scope| {
        let (ready, receiver) = std::sync::mpsc::sync_channel(1);
        let watcher = scope.spawn(move || {
            ready.send(()).unwrap();
            camera
                .watch(Some(3_200), Some(8))
                .expect("watch moving fixture")
        });
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watch caller started");
        // Cadence is 400ms; leave several ticks for both transitions.
        std::thread::sleep(Duration::from_millis(600));
        window.paint(SECOND);
        wait_for(|| color_matches(screen_pixel(window.rect()), SECOND).then_some(()));
        std::thread::sleep(Duration::from_millis(900));
        window.paint(FIRST);
        wait_for(|| color_matches(screen_pixel(window.rect()), FIRST).then_some(()));
        watcher.join().expect("join watch caller")
    })
}

#[test]
fn must_pass_watch_strip_shows_motion_in_window() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    let before = current(&camera, &window, FIRST);
    assert!(color_matches(screen_pixel(window.rect()), FIRST));
    let watch = record_watch_motion(&camera, &window);
    assert_watch_frame_order(&watch);
    let colors = watch_window_colors(&watch, &before, window.rect());
    assert!(
        color_matches(colors[0], FIRST),
        "first cell must sample immediately: {colors:?}"
    );
    let second = colors
        .iter()
        .position(|&color| color_matches(color, SECOND))
        .expect("strip must retain SECOND, not repeat one current screenshot");
    assert!(second > 0, "FIRST must precede SECOND");
    assert!(
        colors[second + 1..]
            .iter()
            .any(|&color| color_matches(color, FIRST)),
        "strip must retain the return to FIRST after SECOND: {colors:?}"
    );
    assert!(watch.frames[second].seq > before.seq);
    window.assert_not_foreground();
}

fn assert_watch_bounds(camera: &Camera, requested_cap: usize, cap: usize, minimum_ms: u64) {
    let started = Instant::now();
    let watch = camera
        .watch(Some(5_000), Some(requested_cap))
        .expect("bounded watch");
    let elapsed = started.elapsed();
    assert!(
        elapsed <= Duration::from_millis(1_600),
        "900ms cap exceeded: {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(minimum_ms),
        "watch skipped its cadence: {elapsed:?}"
    );
    assert_watch_frame_order(&watch);
    assert!(
        watch.frames.len() <= cap,
        "frame cap ignored: {}",
        watch.frames.len()
    );
    assert!(watch.cols * watch.rows >= watch.frames.len());
    assert!(
        watch.ms_elapsed <= 1_600,
        "reported duration must also be bounded"
    );
}

#[test]
fn must_pass_watch_returns_within_ms_and_frame_cap() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera_with(
        CameraConfig {
            fps: 10,
            wait_max_ms: 900,
            watch_max_frames: 4,
            ..CameraConfig::default()
        },
        None,
    );
    current(&camera, &window, FIRST);
    with_watch_paints(&window, || {
        // First exercise the request cap, then the configured cap and ms clamp.
        for (requested_cap, effective_cap, minimum_ms) in [(3, 3, 500), (99, 4, 550)] {
            assert_watch_bounds(&camera, requested_cap, effective_cap, minimum_ms);
        }
        let started = Instant::now();
        let single = camera
            .watch(Some(5_000), Some(1))
            .expect("one immediate sample fills the cap");
        assert_eq!(single.frames.len(), 1);
        assert!(
            started.elapsed() < Duration::from_millis(750),
            "full cap must return before the 900ms deadline"
        );
        assert_watch_frame_order(&single);
    });
    window.assert_not_foreground();
}

#[test]
fn must_fail_watch_not_running() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = Camera::default();
    let assert_not_running = || {
        let started = Instant::now();
        let error = camera
            .watch(Some(5_000), Some(2))
            .expect_err("no watch image without a running camera");
        assert!(
            matches!(error, synapse_livefeed::LivefeedError::NotRunning),
            "wrong typed error: {error:?}"
        );
        assert!(error.to_string().contains("LIVEFEED_NOT_RUNNING"));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "stopped watch must not wait"
        );
    };
    assert_not_running();
    camera
        .start(CameraConfig::default(), None)
        .expect("start positive control");
    current(&camera, &window, FIRST);
    let live = camera
        .watch(Some(100), Some(1))
        .expect("running watch positive control");
    let image = image::load_from_memory(&live.image_jpeg).expect("positive control JPEG");
    assert!(image.width() > 0 && image.height() > 0);
    assert_eq!(live.frames.len(), 1);
    camera.stop().expect("stop before stale-watch check");
    assert_not_running();
}

fn assert_watch_grid_dimensions(watch: &Watch, before: &Look) {
    assert_watch_frame_order(watch);
    assert_eq!(
        watch.frames.len(),
        3,
        "repainting must exercise a non-square grid"
    );
    assert_eq!(
        (watch.cols, watch.rows),
        (2, 2),
        "ceil(sqrt(3)) columns and ceil(3/2) rows"
    );
    assert_eq!(
        watch.cell_w.max(watch.cell_h),
        240,
        "configured cell long edge"
    );
    let strip = image::load_from_memory(&watch.image_jpeg).expect("grid JPEG");
    assert_eq!(
        (strip.width(), strip.height()),
        (
            2 * watch.cell_w + WATCH_SEPARATOR,
            2 * watch.cell_h + WATCH_SEPARATOR,
        ),
        "2px separators must be included between cells, without an outer border"
    );
    let scale = f64::from(watch.cell_w) / f64::from(before.width);
    assert!(
        (f64::from(watch.cell_h) - f64::from(before.height) * scale).abs() <= 1.0,
        "cells must preserve the monitor aspect ratio"
    );
}

#[test]
fn must_pass_watch_grid_layout() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera_with(
        CameraConfig {
            fps: 10,
            watch_max_frames: 3,
            watch_cell_long_edge: 240,
            ..CameraConfig::default()
        },
        None,
    );
    let before = current(&camera, &window, FIRST);
    with_watch_paints(&window, || {
        let watch = camera.watch(Some(1_500), Some(3)).expect("three-cell grid");
        assert_watch_grid_dimensions(&watch, &before);
        let colors = watch_window_colors(&watch, &before, window.rect());
        assert!(
            color_matches(colors[0], FIRST),
            "first cell must contain the fixture at its scaled position"
        );
    });
    window.assert_not_foreground();
}
