//! #246: real GIF worker retry/publication and native screenshot fallback entry.
use super::*;
use std::{cell::Cell, io::BufReader, path::Path};

use image::{AnimationDecoder, Rgba, codecs::gif::GifDecoder};
use synapse_capture::{CaptureError, CapturedBgraBitmap, CapturedWindowBgraBitmap};
use tokio_util::sync::CancellationToken;

use super::super::capture_gif::capture_gif_to_file_for_test;
use crate::m1::CaptureGifResponse;

fn params(path: &Path) -> CaptureGifParams {
    CaptureGifParams {
        path: path.to_string_lossy().into_owned(),
        duration_ms: Some(400),
        interval_ms: Some(100),
        window_hwnd: Some(246),
        max_long_edge: Some(2),
        overwrite: false,
    }
}

fn frame(bgra: [u8; 4]) -> CapturedWindowBgraBitmap {
    CapturedWindowBgraBitmap {
        bitmap: CapturedBgraBitmap {
            region: Rect {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            width: 2,
            height: 2,
            bytes: bgra.repeat(4),
        },
        capture_backend: "injected-246",
        capture_attempts: 1,
        capture_retry_count: 0,
        capture_elapsed_ms: 0,
        capture_retry_backoff_ms: 0,
    }
}

fn invalid_frame() -> CaptureError {
    CaptureError::TargetInvalid {
        detail: "unchanged target backend failure 246".into(),
    }
}

fn identity_unless_lost(lost: &Cell<bool>) -> Result<(i64, u32), ErrorData> {
    if lost.get() {
        Err(mcp_error(
            error_codes::CAPTURE_TARGET_INVALID,
            "target gone 246",
        ))
    } else {
        Ok((246, 10))
    }
}

fn code(error: &ErrorData) -> Option<&str> {
    error.data.as_ref()?.get("code")?.as_str()
}

fn assert_empty(dir: &Path) -> anyhow::Result<()> {
    assert_eq!(
        std::fs::read_dir(dir)?.count(),
        0,
        "no final or temporary artifact"
    );
    Ok(())
}

fn decoded(path: &Path, response: &CaptureGifResponse) -> anyhow::Result<Vec<image::Frame>> {
    assert_eq!(response.bytes_written, std::fs::metadata(path)?.len());
    assert!(response.bytes_written > 0);
    let decoder = GifDecoder::new(BufReader::new(std::fs::File::open(path)?))?;
    let frames = decoder.into_frames().collect_frames()?;
    assert_eq!(frames.len(), response.frames_captured);
    assert_eq!((response.width, response.height), (2, 2));
    Ok(frames)
}

#[test]
fn must_pass_gif_target_invalid_frame_with_unchanged_identity_does_not_abort_recording()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("retry.gif");
    assert_empty(dir.path())?;
    let mut captures = 0;
    let response = capture_gif_to_file_for_test(
        params(&path),
        path.clone(),
        246,
        CancellationToken::new(),
        || Ok((246, 10)),
        || {
            captures += 1;
            match captures {
                1 => Ok(frame([0, 0, 255, 255])),
                2 => Err(invalid_frame()),
                _ => Ok(frame([255, 0, 0, 255])),
            }
        },
    )?
    .0;
    assert!(
        captures >= 3,
        "must capture a frame after the backend failure"
    );
    let frames = decoded(&path, &response)?;
    assert_eq!(
        frames.len(),
        captures - 1,
        "failed frame must not be retained"
    );
    assert!(frames.len() >= 2);
    for (index, frame) in frames.iter().enumerate() {
        let color = if index == 0 {
            Rgba([255, 0, 0, 255])
        } else {
            Rgba([0, 0, 255, 255])
        };
        assert!(frame.buffer().pixels().all(|pixel| *pixel == color));
    }
    Ok(())
}

#[test]
fn must_fail_all_frames_target_invalid_stable_identity_reports_no_observed_delta()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("empty.gif");
    assert_empty(dir.path())?;
    let mut captures = 0;
    let error = capture_gif_to_file_for_test(
        params(&path),
        path.clone(),
        246,
        CancellationToken::new(),
        || Ok((246, 10)),
        || {
            captures += 1;
            Err(invalid_frame())
        },
    )
    .err()
    .expect("zero verified frames must fail");
    assert!(captures > 0);
    assert_eq!(code(&error), Some(error_codes::ACTION_NO_OBSERVED_DELTA));
    assert!(
        error
            .message
            .contains("unchanged target backend failure 246")
    );
    assert!(!path.exists());
    assert_empty(dir.path())
}

#[test]
fn must_fail_live_pid_replacement_aborts_with_target_invalid() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("replaced.gif");
    assert_empty(dir.path())?;
    let mut reads = 0;
    let mut captures = 0;
    let error = capture_gif_to_file_for_test(
        params(&path),
        path.clone(),
        246,
        CancellationToken::new(),
        || {
            reads += 1;
            // Initial identity + first frame's pre/post reads stay stable.
            Ok((246, if reads <= 3 { 10 } else { 11 }))
        },
        || {
            captures += 1;
            Ok(frame([0, 0, 255, 255]))
        },
    )
    .err()
    .expect("live PID replacement must refuse publication");
    assert_eq!(captures, 1, "one verified frame before substitution");
    assert_eq!(code(&error), Some(error_codes::CAPTURE_TARGET_INVALID));
    assert!(!path.exists());
    assert_empty(dir.path())
}

#[test]
fn must_pass_identity_unavailable_after_verified_frame_keeps_partial_gif() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("partial.gif");
    assert_empty(dir.path())?;
    let lost = Cell::new(false);
    let mut captures = 0;
    let response = capture_gif_to_file_for_test(
        params(&path),
        path.clone(),
        246,
        CancellationToken::new(),
        || identity_unless_lost(&lost),
        || {
            captures += 1;
            if captures == 1 {
                Ok(frame([0, 0, 255, 255]))
            } else {
                lost.set(true);
                Err(invalid_frame())
            }
        },
    )?
    .0;
    assert!(
        lost.get(),
        "identity must become unavailable after capture failure"
    );
    assert_eq!(captures, 2);
    assert_eq!(response.frames_captured, 1);
    assert_eq!(response.frames_requested, 4);
    let frames = decoded(&path, &response)?;
    assert!(
        frames[0]
            .buffer()
            .pixels()
            .all(|pixel| *pixel == Rgba([255, 0, 0, 255]))
    );
    assert_eq!(
        std::fs::read_dir(dir.path())?.count(),
        1,
        "only final GIF remains"
    );
    Ok(())
}

#[test]
fn must_fail_native_entry_stable_identity_preserves_backend_error_without_hidden_desktop()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let service = SynapseService::try_new_isolated_for_tests(root.path())?;
    let output = tempfile::tempdir()?;
    let path = output.path().join("native.png");
    let params = CaptureScreenshotParams {
        path: path.to_string_lossy().into_owned(),
        region: None,
        window_hwnd: Some(246),
        overwrite: false,
        max_pixels: None,
        max_long_edge: None,
    };
    let region = Rect {
        x: 0,
        y: 0,
        w: 2,
        h: 2,
    };
    let failures: [fn() -> CaptureError; 3] = [
        || CaptureError::WgcFrameTimeout {
            detail: "native timeout 246".into(),
        },
        || CaptureError::PrintWindowDisabled {
            detail: "native disabled 246".into(),
        },
        invalid_frame,
    ];
    for make_error in failures {
        assert_native_error(&service, &params, region, output.path(), make_error)?;
    }
    Ok(())
}

fn assert_native_error(
    service: &SynapseService,
    params: &CaptureScreenshotParams,
    region: Rect,
    output: &Path,
    make_error: fn() -> CaptureError,
) -> anyhow::Result<()> {
    assert_empty(output)?;
    let native = make_error();
    let expected_code = native.code();
    let expected = capture_target_window_screenshot_error(246, region, None, make_error());
    let mut captures = 0;
    let error = service
        .capture_target_window_screenshot_with_fallback_with(
            params,
            None,
            246,
            region,
            None,
            || Ok((246, 10)),
            || {
                captures += 1;
                Err(native)
            },
        )
        .expect_err("native entry must preserve backend failure");
    assert_eq!(captures, 1, "real native capture dependency must run");
    assert_eq!(code(&error), Some(expected_code));
    assert_eq!(
        serde_json::to_value(error)?,
        serde_json::to_value(expected)?
    );
    assert!(!Path::new(&params.path).exists());
    assert_empty(output)
}
