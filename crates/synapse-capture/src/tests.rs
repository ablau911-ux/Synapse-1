use std::sync::Mutex;
// `Ordering`, `thread`, and `Duration` are only used by the Windows-only capture
// loop tests below (real capture does not exist off Windows after the synthetic
// mock removal).
#[cfg(windows)]
use std::{sync::atomic::Ordering, thread, time::Duration};

use proptest::prelude::*;
use synapse_core::{Point, error_codes};

use super::*;

static ENV_LOCK: Mutex<()> = Mutex::new(());
// Only the Windows-only capture loop tests serialize on this lock.
#[cfg(windows)]
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

// The previous `captured_frame_synthetic_shape_is_stable` and
// `captured_frame_drop_loop_is_raii_safe_for_synthetic_texture` tests exercised
// the non-Windows `CapturedFrame::synthetic` mock, which has been removed. The
// non-Windows capture path is now covered by the fail-loud test
// `must_fail_non_windows_capture_is_graphics_api_unsupported` below (it asserts
// `GraphicsApiUnsupported` instead of fabricating frames).

#[cfg(not(windows))]
#[test]
fn must_fail_non_windows_capture_is_graphics_api_unsupported() {
    let region = synapse_core::Rect {
        x: 0,
        y: 0,
        w: 8,
        h: 8,
    };
    let error = match screen_region_to_bgra_bitmap(region) {
        Ok(bitmap) => panic!("off Windows capture must fail loud, got {bitmap:?}"),
        Err(error) => error,
    };
    assert_eq!(error.code(), error_codes::CAPTURE_GRAPHICS_API_UNSUPPORTED);
}

#[cfg(windows)]
#[test]
fn must_pass_captured_frame_drop_loop_produces_owned_cpu_buffer() -> Result<(), CaptureError> {
    let _guard = CAPTURE_LOCK
        .lock()
        // Recover from poison instead of panicking: these serialization guards
        // only prevent two capture loops running at once, they guard no shared
        // data. Panicking here turned ONE failing capture test into a cascade of
        // "capture lock poisoned" failures in every later capture test.
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let handle = spawn_capture_loop(CaptureConfig {
        min_update_interval_ms: 16,
        dirty_region_only: false,
        ..CaptureConfig::default()
    })?;
    let rx = handle.receiver();
    let mut queried = 0_u32;

    // DXGI Desktop Duplication is change-driven: `acquire_next_frame` returns
    // `DxgiError::Timeout` (DXGI_ERROR_WAIT_TIMEOUT) and the capture loop pushes
    // NO frame while the desktop is static — re-delivering identical frames would
    // be wasteful, so that is the correct product behavior. The synthetic frame
    // source was removed, so the only source here is the real, change-gated
    // desktop. A fixed `for _ in 0..1000 { recv_timeout }` therefore hangs and
    // fails on any static desktop (e.g. an unattended automated run) even though
    // capture is healthy — it captured the first frame fine. Bound the loop by a
    // wall-clock budget instead, and exercise the actual invariant under test:
    // each captured frame has an owned, tightly packed CPU buffer matching the
    // reported width/height. The first
    // `acquire_next_frame` after duplication start always returns the current
    // desktop, so at least one frame is guaranteed; on an active desktop the loop
    // still exercises the drop path across up to 1000 frames.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut idle_slices = 0_u32;
    while queried < 1_000 && std::time::Instant::now() < deadline {
        let Ok(frame) = rx.recv_timeout(Duration::from_millis(250)) else {
            idle_slices = idle_slices.saturating_add(1);
            // Once capture is proven (>=1 frame), a static desktop simply
            // stops producing frames; stop after ~1s of quiet rather than
            // burning the whole budget.
            if queried >= 1 && idle_slices >= 4 {
                break;
            }
            continue;
        };
        idle_slices = 0;
        if queried == 0 {
            println!(
                "owned_frame frame_seq={} width={} height={} row_stride={} bytes_per_pixel={} byte_len={}",
                frame.frame_seq,
                frame.width,
                frame.height,
                frame.pixels.row_stride_bytes,
                frame.pixels.bytes_per_pixel,
                frame.pixels.bytes.len()
            );
        }
        let bytes_per_pixel = usize::from(frame.pixels.bytes_per_pixel);
        assert!(bytes_per_pixel > 0);
        let row_len = usize::try_from(frame.width)
            .ok()
            .and_then(|width| width.checked_mul(bytes_per_pixel))
            .expect("frame row length should fit usize");
        assert_eq!(frame.pixels.row_stride_bytes, row_len);
        let expected_len = row_len
            .checked_mul(usize::try_from(frame.height).expect("height should fit usize"))
            .expect("frame byte length should fit usize");
        assert_eq!(frame.pixels.bytes.len(), expected_len);
        queried = queried.saturating_add(1);
    }

    let stats = handle.stats();
    println!(
        "after owned_frame_loop queried={} captured={} dropped={} priority={:?}",
        queried,
        stats.frames_captured(),
        stats.frames_dropped(),
        stats.thread_priority()
    );
    handle.stop()?;
    assert!(
        queried >= 1,
        "expected at least the guaranteed first DXGI frame, got {queried}"
    );
    Ok(())
}

#[test]
fn must_pass_force_dxgi_env_value_selects_dxgi_backend() {
    let config = CaptureConfig {
        backend_preference: CaptureBackendPreference::from_force_dxgi_value(Some("1")),
        ..CaptureConfig::default()
    };
    assert_eq!(config.selected_backend(), CaptureBackend::DxgiDuplication);
}

#[test]
fn must_fail_unset_or_false_force_dxgi_value_keeps_default_backend() {
    for value in [None, Some(""), Some("0"), Some("false"), Some("no")] {
        let config = CaptureConfig {
            backend_preference: CaptureBackendPreference::from_force_dxgi_value(value),
            ..CaptureConfig::default()
        };
        assert_eq!(
            config.backend_preference,
            CaptureBackendPreference::Auto,
            "value={value:?}"
        );
        assert_eq!(
            config.selected_backend(),
            CaptureBackend::GraphicsCaptureApi,
            "value={value:?} must keep the default backend"
        );
    }
}

#[test]
fn must_pass_force_dxgi_env_var_selects_dxgi_backend() {
    let _guard = ENV_LOCK
        .lock()
        // Recover from poison rather than cascade (see capture-lock note).
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = std::env::var_os("SYNAPSE_CAPTURE_FORCE_DXGI");
    println!(
        "before env_dxgi previous={:?} selected_backend={:?}",
        previous,
        CaptureConfig::default().selected_backend()
    );

    // SAFETY: this test serializes access with ENV_LOCK and restores the
    // prior value before returning.
    unsafe {
        std::env::set_var("SYNAPSE_CAPTURE_FORCE_DXGI", "1");
    }
    let config = CaptureConfig::default().with_env_backend();
    println!(
        "after env_dxgi value=1 selected_backend={:?}",
        config.selected_backend()
    );
    assert_eq!(config.selected_backend(), CaptureBackend::DxgiDuplication);

    // SAFETY: same ENV_LOCK serialization as above.
    unsafe {
        match previous {
            Some(value) => std::env::set_var("SYNAPSE_CAPTURE_FORCE_DXGI", value),
            None => std::env::remove_var("SYNAPSE_CAPTURE_FORCE_DXGI"),
        }
    }
}

#[test]
fn must_fail_auto_backend_falls_back_only_for_graphics_unsupported() {
    let unsupported = CaptureError::GraphicsApiUnsupported {
        detail: "synthetic unsupported".to_owned(),
    };
    println!(
        "before fallback preference={:?} error_code={}",
        CaptureBackendPreference::Auto,
        unsupported.code()
    );
    assert!(should_fallback_to_dxgi(
        CaptureBackendPreference::Auto,
        &unsupported
    ));
    assert_eq!(
        backend_after_fallback(CaptureBackendPreference::Auto, &unsupported),
        CaptureBackend::DxgiDuplication
    );
    println!(
        "after fallback effective_backend={:?}",
        backend_after_fallback(CaptureBackendPreference::Auto, &unsupported)
    );

    let invalid = CaptureError::TargetInvalid {
        detail: "bad hwnd".to_owned(),
    };
    assert!(!should_fallback_to_dxgi(
        CaptureBackendPreference::Auto,
        &invalid
    ));

    let printwindow_disabled = CaptureError::PrintWindowDisabled {
        detail: "target-process WM_PRINT rendering disabled".to_owned(),
    };
    assert!(!should_fallback_to_dxgi(
        CaptureBackendPreference::Auto,
        &printwindow_disabled
    ));
    assert_eq!(
        printwindow_disabled.code(),
        error_codes::CAPTURE_PRINTWINDOW_DISABLED
    );
}

#[test]
fn must_fail_forced_graphics_capture_never_falls_back_to_dxgi() {
    // Only Auto may fall back; an explicit preference is honoured even when its
    // backend reports GraphicsApiUnsupported.
    let unsupported = CaptureError::GraphicsApiUnsupported {
        detail: "synthetic unsupported".to_owned(),
    };
    assert!(!should_fallback_to_dxgi(
        CaptureBackendPreference::GraphicsCaptureApi,
        &unsupported
    ));
    assert_eq!(
        backend_after_fallback(CaptureBackendPreference::GraphicsCaptureApi, &unsupported),
        CaptureBackend::GraphicsCaptureApi
    );
    assert!(!should_fallback_to_dxgi(
        CaptureBackendPreference::DxgiDuplication,
        &unsupported
    ));
    assert_eq!(
        backend_after_fallback(CaptureBackendPreference::DxgiDuplication, &unsupported),
        CaptureBackend::DxgiDuplication
    );
}

#[test]
fn must_fail_invalid_hwnd_surfaces_capture_target_invalid() {
    let config = CaptureConfig {
        target: CaptureTarget::Window { hwnd: 0 },
        ..CaptureConfig::default()
    };
    println!("before invalid_hwnd target={:?}", config.target);

    let err = resolve_capture_target(&config)
        .err()
        .unwrap_or_else(|| panic!("invalid hwnd should fail"));
    println!("after invalid_hwnd error_code={}", err.code());
    assert_eq!(err.code(), error_codes::CAPTURE_TARGET_INVALID);
}

#[test]
fn must_fail_dxgi_backend_rejects_window_targets_before_thread_spawn() {
    let config = CaptureConfig {
        target: CaptureTarget::Window { hwnd: 1 },
        backend_preference: CaptureBackendPreference::DxgiDuplication,
        ..CaptureConfig::default()
    };
    println!(
        "before dxgi_window target={:?} selected_backend={:?}",
        config.target,
        config.selected_backend()
    );

    let err = resolve_capture_target(&config)
        .err()
        .unwrap_or_else(|| panic!("DXGI window target should fail"));
    println!("after dxgi_window error_code={} error={err}", err.code());
    assert_eq!(err.code(), error_codes::CAPTURE_TARGET_INVALID);
    assert!(err.to_string().contains("monitor targets only"));
}

fn synthetic_frame(frame_seq: u64) -> CapturedFrame {
    CapturedFrame {
        pixels: CapturedFrameBuffer {
            bytes: vec![0; 4],
            row_stride_bytes: 4,
            bytes_per_pixel: 4,
        },
        width: 1,
        height: 1,
        format: DxgiFormat::Bgra8,
        captured_at: std::time::Instant::now(),
        frame_seq,
        dirty_region: None,
    }
}

/// Drives the production `push_frame` (the capture loop's only enqueue path)
/// with a channel of the production capacity and no consumer: three frames in,
/// the oldest is dropped, the receiver yields exactly [2, 3].
#[test]
fn must_fail_capture_channel_capacity_is_exactly_two_and_drops_oldest() -> Result<(), CaptureError>
{
    let (tx, rx) = crossbeam::channel::bounded(CAPTURE_CHANNEL_CAPACITY);
    let ctx = crate::controller::CaptureThreadContext {
        tx,
        rx: rx.clone(),
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        stats: std::sync::Arc::new(CaptureStats::default()),
    };

    for frame_seq in 1..=3 {
        crate::controller::push_frame(&ctx, synthetic_frame(frame_seq))?;
    }

    assert_eq!(rx.len(), CAPTURE_CHANNEL_CAPACITY);
    let delivered: Vec<u64> = rx.try_iter().map(|frame| frame.frame_seq).collect();
    assert_eq!(
        delivered,
        [2, 3],
        "the OLDEST frame must be the one dropped"
    );
    assert_eq!(ctx.stats.frames_captured(), 3);
    assert_eq!(ctx.stats.frames_dropped(), 1);
    Ok(())
}

// Windows-only: the capture thread now fails loudly off Windows (no real
// backend), so `stop()` would surface that error. Off Windows there is no
// capture thread priority to read back.
#[cfg(windows)]
#[test]
fn must_pass_capture_thread_priority_is_recorded() -> Result<(), CaptureError> {
    let _guard = CAPTURE_LOCK
        .lock()
        // Recover from poison instead of panicking: these serialization guards
        // only prevent two capture loops running at once, they guard no shared
        // data. Panicking here turned ONE failing capture test into a cascade of
        // "capture lock poisoned" failures in every later capture test.
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let handle = spawn_capture_loop(CaptureConfig {
        min_update_interval_ms: 1,
        ..CaptureConfig::default()
    })?;
    let stats = handle.stats();
    println!("before priority_readback={:?}", stats.thread_priority());
    // Poll with a deadline: the capture thread records its priority on start.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while stats.thread_priority() != CaptureThreadPriority::TimeCritical
        && std::time::Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(5));
    }
    let priority = stats.thread_priority();
    println!("after priority_readback={priority:?}");
    assert_eq!(priority, CaptureThreadPriority::TimeCritical);
    handle.stop()
}

#[test]
fn must_pass_coordinate_transform_manual_edge_cases_round_trip() {
    let cases = [
        (Point { x: 0, y: 0 }, Point { x: 0, y: 0 }),
        (
            Point {
                x: 100_000,
                y: -100_000,
            },
            Point {
                x: -10_000,
                y: 10_000,
            },
        ),
        (
            Point {
                x: -100_000,
                y: 100_000,
            },
            Point {
                x: 10_000,
                y: -10_000,
            },
        ),
    ];

    for (point, origin) in cases {
        println!("before transform point={point:?} origin={origin:?}");
        let screen = window_to_screen_with_origin(point, origin);
        let round_trip = screen_to_window_with_origin(screen, origin);
        println!("after transform screen={screen:?} round_trip={round_trip:?}");
        // Window-to-screen adds the window origin (a sign-inverted pair would
        // still round-trip, so pin the screen value itself).
        assert_eq!(
            screen,
            Point {
                x: point.x + origin.x,
                y: point.y + origin.y,
            }
        );
        assert_eq!(round_trip, point);
    }
}

// Windows-only: switching sessions joins capture threads, which now return the
// honest "unsupported" error off Windows instead of mock success.
#[cfg(windows)]
#[test]
fn must_pass_switching_capture_target_stops_previous_session() -> Result<(), CaptureError> {
    let _guard = CAPTURE_LOCK
        .lock()
        // Recover from poison instead of panicking: these serialization guards
        // only prevent two capture loops running at once, they guard no shared
        // data. Panicking here turned ONE failing capture test into a cascade of
        // "capture lock poisoned" failures in every later capture test.
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut controller = CaptureController::new();
    assert_eq!(controller.switch_to(CaptureConfig::default())?, 1);
    let first_stop = controller.active().map_or_else(
        || panic!("capture handle should be active"),
        |handle| handle.stop.clone(),
    );
    assert_eq!(
        controller.switch_to(CaptureConfig {
            target: CaptureTarget::Monitor { monitor_index: 0 },
            ..CaptureConfig::default()
        })?,
        2
    );
    assert!(first_stop.load(Ordering::Relaxed));
    Ok(())
}

proptest! {
    #[test]
    fn must_pass_coordinate_transform_origin_round_trip(
        x in -100_000_i32..100_000,
        y in -100_000_i32..100_000,
        ox in -10_000_i32..10_000,
        oy in -10_000_i32..10_000,
    ) {
        let point = Point { x, y };
        let origin = Point { x: ox, y: oy };
        let screen = window_to_screen_with_origin(point, origin);
        prop_assert_eq!(screen, Point { x: x + ox, y: y + oy });
        prop_assert_eq!(screen_to_window_with_origin(screen, origin), point);
    }
}

#[test]
fn must_pass_dpi_awareness_readback_matches_platform() -> Result<(), CaptureError> {
    let before = is_per_monitor_v2_dpi_aware();
    let status = init_process_dpi_awareness()?;
    let after = is_per_monitor_v2_dpi_aware();
    println!("dpi_readback before={before} status={status:?} after={after}");
    if cfg!(windows) {
        assert!(after);
    } else {
        assert_eq!(status, DpiAwarenessStatus::Unsupported);
        assert!(!after);
    }
    Ok(())
}
