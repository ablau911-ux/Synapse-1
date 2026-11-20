#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(windows)]

mod support;

use std::time::{Duration, Instant};

use synapse_core::Rect;
use synapse_livefeed::camera::{Camera, Look};
use synapse_livefeed::config::CameraConfig;

use support::monitors::{Monitor, monitors};
use support::window::{Window, screen_pixel, thread_description, thread_ids};
use support::{CAMERA_TEST_LOCK, FIRST, SECOND, color_matches, pixel, wait_for};

struct RunningCamera(Camera);

impl std::ops::Deref for RunningCamera {
    type Target = Camera;

    fn deref(&self) -> &Camera {
        &self.0
    }
}

impl Drop for RunningCamera {
    fn drop(&mut self) {
        let _ = self.0.stop();
    }
}

fn start_camera() -> RunningCamera {
    let config = CameraConfig {
        fps: 10,
        look_long_edge: 320,
        ..CameraConfig::default()
    };
    start_camera_with(config, None)
}

fn start_camera_with(config: CameraConfig, monitor: Option<u32>) -> RunningCamera {
    let camera = Camera::default();
    camera
        .start(config, monitor)
        .expect("start primary-monitor camera");
    assert!(camera.status().running);
    RunningCamera(camera)
}

fn current(camera: &Camera, window: &Window, color: [u8; 3]) -> Look {
    let rect = window.rect();
    wait_for(|| {
        let look = camera.look().ok()?;
        let image = image::load_from_memory(&look.image_jpeg)
            .expect("look JPEG")
            .to_rgb8();
        let x = (f64::from(rect.x + rect.w / 2 - look.origin_x) * f64::from(image.width())
            / f64::from(look.width)) as i32;
        let y = (f64::from(rect.y + rect.h / 2 - look.origin_y) * f64::from(image.height())
            / f64::from(look.height)) as i32;
        color_matches(pixel(&image, x, y), color).then_some(look)
    })
}

fn contains(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn window_interior(rect: Rect) -> Rect {
    let tile = i32::try_from(CameraConfig::default().tile_px).unwrap();
    let interior = Rect {
        x: rect.x + tile,
        y: rect.y + tile,
        w: rect.w - 2 * tile,
        h: rect.h - 2 * tile,
    };
    assert!(
        interior.w > 0 && interior.h > 0,
        "fixture must contain interior tiles"
    );
    interior
}

#[test]
fn must_pass_look_returns_current_screen() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    wait_for(|| color_matches(screen_pixel(window.rect()), FIRST).then_some(()));
    let camera = start_camera();
    let before = current(&camera, &window, FIRST);
    window.paint(SECOND);
    wait_for(|| color_matches(screen_pixel(window.rect()), SECOND).then_some(()));
    let after = current(&camera, &window, SECOND);
    assert!(
        after.seq > before.seq,
        "look must not return the previous image"
    );
    assert!(after.ts_ms >= before.ts_ms);
    assert_eq!(format!("{:?}", after.screen_state), "Live");
    let image = image::load_from_memory(&after.image_jpeg).unwrap();
    assert_eq!(image.width().max(image.height()), 320);
    let status = camera.status();
    assert_eq!((after.width, after.height), (status.width, status.height));
    let expected_scale = 320.0 / f64::from(after.width.max(after.height));
    assert!((f64::from(after.scale) - expected_scale).abs() < 0.01);
    window.assert_not_foreground();
}

#[test]
fn must_pass_changes_returns_crop_of_changed_window_area() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    let before = current(&camera, &window, FIRST);
    window.paint(SECOND);
    let after = current(&camera, &window, SECOND);
    let changes = camera
        .changes(before.seq, Some(0))
        .expect("changes since first color");
    assert_eq!(changes.since, before.seq);
    assert!(changes.seq >= after.seq);
    assert!(!changes.since_evicted);
    assert!(!changes.too_many);
    assert_eq!(format!("{:?}", changes.screen_state), "Live");
    let rect = window.rect();
    let (x, y) = (rect.x + rect.w / 2, rect.y + rect.h / 2);
    let region = changes
        .regions
        .iter()
        .find(|r| contains(r.rect, x, y))
        .expect("changed region must include the window's screen position");
    let crop = image::load_from_memory(&region.crop_jpeg)
        .expect("region JPEG")
        .to_rgb8();
    assert_eq!(
        (crop.width(), crop.height()),
        (region.rect.w as u32, region.rect.h as u32)
    );
    assert!(color_matches(
        pixel(&crop, x - region.rect.x, y - region.rect.y),
        SECOND
    ));
    assert!(
        color_matches(screen_pixel(rect), SECOND),
        "independent screen readback"
    );
}

#[test]
fn must_pass_changes_empty_when_nothing_changed() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    let look = current(&camera, &window, FIRST);
    let rect = window.rect();
    let interior = window_interior(rect);
    // Static WGC desktops need not deliver more frames; observe an idle interval.
    std::thread::sleep(Duration::from_millis(350));
    let changes = camera
        .changes(look.seq, Some(0))
        .expect("changes over unchanged window");
    assert_eq!(changes.since, look.seq);
    assert!(!changes.since_evicted);
    assert!(
        color_matches(screen_pixel(rect), FIRST),
        "window stayed unchanged"
    );
    assert!(
        changes
            .regions
            .iter()
            .all(|r| !intersects(r.rect, interior)),
        "unchanged window interior must not be reported: {:?}",
        changes.regions.iter().map(|r| r.rect).collect::<Vec<_>>()
    );
    // Positive control: a camera that always returns no regions must fail too.
    window.paint(SECOND);
    current(&camera, &window, SECOND);
    let changed = camera.changes(changes.seq, Some(0)).unwrap();
    assert!(!changed.since_evicted);
    assert!(changed.regions.iter().any(|r| intersects(r.rect, interior)));
}

#[test]
fn must_pass_wait_returns_when_change_happens() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    let before = current(&camera, &window, FIRST);
    let rect = window.rect();
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let waiting_camera: &Camera = &camera;
        let waiter = scope.spawn(move || {
            ready_tx.send(()).unwrap();
            let changes = wait_for_window_change(waiting_camera, before.seq, rect);
            tx.send(changes).expect("send waiter result");
        });
        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("waiter started");
        // Unrelated repainting may wake changes at any point before our paint.
        std::thread::sleep(Duration::from_millis(250));
        let painted = Instant::now();
        window.paint(SECOND);
        let changes = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("window paint wakes waiter before its 5 second timeout");
        assert!(painted.elapsed() < Duration::from_secs(3));
        assert!(changes.seq > before.seq);
        assert!(
            changes
                .regions
                .iter()
                .any(|r| { contains(r.rect, rect.x + rect.w / 2, rect.y + rect.h / 2) }),
            "wait result must contain the changed window, not just a new cursor"
        );
        assert!(color_matches(screen_pixel(rect), SECOND));
        waiter.join().expect("waiter joined");
    });
}

fn wait_for_window_change(
    camera: &Camera,
    mut cursor: u64,
    rect: Rect,
) -> synapse_livefeed::camera::Changes {
    let deadline = Instant::now() + Duration::from_secs(4);
    let (x, y) = (rect.x + rect.w / 2, rect.y + rect.h / 2);
    loop {
        assert!(
            Instant::now() < deadline,
            "wait never returned our painted window"
        );
        let changes = camera
            .changes(cursor, Some(5_000))
            .expect("wait for changes");
        assert_eq!(changes.since, cursor);
        assert!(changes.seq >= cursor);
        assert!(!changes.since_evicted, "wait cursor must remain in history");
        cursor = changes.seq;
        // A merged ambient region may cover the window; require its new pixels.
        if changes.regions.iter().any(|region| {
            if !contains(region.rect, x, y) || region.crop_jpeg.is_empty() {
                return false;
            }
            let crop = image::load_from_memory(&region.crop_jpeg)
                .expect("wait region JPEG")
                .to_rgb8();
            color_matches(pixel(&crop, x - region.rect.x, y - region.rect.y), SECOND)
        }) {
            return changes;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn must_pass_zoom_is_full_resolution() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    let look = current(&camera, &window, FIRST);
    let rect = window.rect();
    let zoom = camera.zoom(rect).expect("zoom own window");
    assert_eq!(zoom.rect, rect);
    assert!(zoom.seq >= look.seq);
    let image = image::load_from_memory(&zoom.image_jpeg).unwrap().to_rgb8();
    assert_eq!(
        (image.width(), image.height()),
        (rect.w as u32, rect.h as u32)
    );
    assert!(look.scale < 1.0, "fixture must exercise a downscaled look");
    assert!(color_matches(pixel(&image, rect.w / 2, rect.h / 2), FIRST));
    assert!(color_matches(screen_pixel(rect), FIRST));
}

#[test]
fn must_fail_zoom_outside_frame() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let camera = start_camera();
    let look = current(&camera, &window, FIRST);
    for (x, y, w, h) in [
        (-1, 0, 1, 1),
        (0, -1, 1, 1),
        (look.width as i32, 0, 1, 1),
        (0, look.height as i32, 1, 1),
        (look.width as i32 - 1, 0, 2, 1),
    ] {
        let error = camera
            .zoom(Rect {
                x: x + look.origin_x,
                y: y + look.origin_y,
                w,
                h,
            })
            .expect_err("outside crop must not clip silently");
        let typed: &dyn std::error::Error = &error;
        assert!(
            typed.to_string().contains("LIVEFEED_BAD_REGION"),
            "wrong error: {error:?}"
        );
        assert!(
            camera.status().running,
            "bad region must not stop the camera"
        );
    }
}

#[test]
fn must_pass_stop_joins_threads() {
    let _lock = CAMERA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let window = Window::spawn();
    let baseline = thread_ids();
    let camera = start_camera();
    current(&camera, &window, FIRST);
    let running = thread_ids();
    let started: Vec<_> = running
        .difference(&baseline)
        .filter_map(|&id| {
            let name = thread_description(id)?;
            (name == "synapse-livefeed" || name.starts_with("synapse-capture"))
                .then_some((id, name))
        })
        .collect();
    for required in ["synapse-livefeed", "synapse-capture"] {
        assert!(
            started.iter().any(|(_, name)| name == required),
            "missing named camera worker {required}: {started:?}"
        );
    }
    camera.stop().expect("stop joins camera workers");
    assert!(!camera.status().running);
    let stopped = thread_ids();
    for (id, name) in started {
        assert!(
            !stopped.contains(&id),
            "camera thread {name} ({id}) survived stop()"
        );
    }
    let error = camera
        .look()
        .expect_err("stopped camera must not return a stale image");
    assert!(
        error.to_string().contains("LIVEFEED_NOT_RUNNING"),
        "wrong error: {error:?}"
    );
}

include!("support/addendum.rs");
include!("support/addendum3.rs");
include!("support/addendum4.rs");
