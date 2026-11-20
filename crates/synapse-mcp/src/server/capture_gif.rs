//! `capture_gif` MCP tool (#1339).
//!
//! Claude-in-Chrome ships `gif_creator` to record interactions as shareable
//! GIFs. Synapse had `demo_record_*` (UIA JSONL for profile authoring) and
//! terminal asciicast, but no VISUAL screen/browser GIF recorder.
//!
//! `capture_gif` records the bound window (or an explicit HWND, or the browser
//! window behind a CDP target) by capturing periodic passive per-window WGC
//! frames over a recording window, then encodes an animated GIF. It is a single
//! synchronous call — no dangling recording state machine — and reports captured
//! vs requested frame counts so there are never silent frame drops. WGC captures
//! occluded/background windows, so recording does not require foreground.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{
    Delay, Frame, RgbaImage,
    codecs::gif::{GifEncoder, Repeat},
};
use rmcp::{RoleServer, model::ErrorCode, service::RequestContext};
use serde_json::json;
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;

use super::output_path_policy::CAPTURE_GIF_PATH_POLICY;
use super::{
    CaptureGifParams, CaptureGifResponse, ErrorData, Json, Parameters, SessionTarget,
    SynapseService, tool, tool_router,
};
use crate::m1::mcp_error;

const DEFAULT_DURATION_MS: u64 = 3_000;
const MAX_DURATION_MS: u64 = 60_000;
const DEFAULT_INTERVAL_MS: u64 = 500;
const MIN_INTERVAL_MS: u64 = 100;
const DEFAULT_MAX_LONG_EDGE: u32 = 800;
const FRAME_TIMEOUT_MS: u64 = 1_500;
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[tool_router(router = capture_gif_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Record the bound window as an animated GIF (Claude gif_creator parity, #1339). Captures periodic passive per-window WGC frames of the session's bound target window — or an explicit window_hwnd, or the browser window behind a CDP tab target — over duration_ms at interval_ms, then encodes an animated GIF to an absolute .gif path. WGC captures occluded/background windows, so recording needs no foreground. Frames are downscaled aspect-preserving to max_long_edge (default 800). Single synchronous call (no recording state machine); reports frames_captured vs frames_requested so frame drops are never silent, and fails loud on zero frames or encode failure. Use capture_screenshot for a single still."
    )]
    pub async fn capture_gif(
        &self,
        params: Parameters<CaptureGifParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<CaptureGifResponse>, ErrorData> {
        let params = params.0;
        let cancel = request_context.ct.clone();
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "capture_gif",
            "tool.invocation kind=capture_gif"
        );

        capture_gif_check_cancelled(&cancel)?;
        capture_gif_timing(&params)?;

        let output_path = capture_gif_output_path(&params.path)?;

        let window_hwnd = self.capture_gif_resolve_window(params.window_hwnd, &request_context)?;

        tokio::task::spawn_blocking(move || {
            capture_gif_to_file_with(
                params,
                output_path,
                window_hwnd,
                cancel,
                || super::m1_tools::capture_window_identity(window_hwnd),
                || synapse_capture::window_full_frame_to_bgra_bitmap(window_hwnd, FRAME_TIMEOUT_MS),
            )
        })
        .await
        .map_err(|error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                error.to_string(),
            )
        })?
    }

    fn capture_gif_resolve_window(
        &self,
        explicit: Option<i64>,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<i64, ErrorData> {
        if let Some(hwnd) = explicit {
            return Ok(hwnd);
        }
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?;
        let target = self.session_target(session_id.as_deref())?;
        match target {
            Some(SessionTarget::Window { hwnd }) => Ok(hwnd),
            Some(SessionTarget::Cdp { window_hwnd, .. }) => Ok(window_hwnd),
            None => Err(mcp_error(
                synapse_core::error_codes::TARGET_NOT_SET,
                "capture_gif requires a window_hwnd or a bound session target (set_target)",
            )),
        }
    }
}

// Test-only bridge: keep runtime visibility private while testing the shared worker
// alongside the native screenshot entry in m1_tools' test module.
#[cfg(all(test, windows))]
pub(super) fn capture_gif_to_file_for_test(
    params: CaptureGifParams,
    output_path: PathBuf,
    window_hwnd: i64,
    cancel: CancellationToken,
    identity: impl FnMut() -> Result<(i64, u32), ErrorData>,
    capture: impl FnMut() -> Result<
        synapse_capture::CapturedWindowBgraBitmap,
        synapse_capture::CaptureError,
    >,
) -> Result<Json<CaptureGifResponse>, ErrorData> {
    capture_gif_to_file_with(params, output_path, window_hwnd, cancel, identity, capture)
}

fn capture_gif_to_file_with(
    params: CaptureGifParams,
    output_path: std::path::PathBuf,
    window_hwnd: i64,
    cancel: tokio_util::sync::CancellationToken,
    mut identity: impl FnMut() -> Result<(i64, u32), ErrorData>,
    mut capture: impl FnMut() -> Result<
        synapse_capture::CapturedWindowBgraBitmap,
        synapse_capture::CaptureError,
    >,
) -> Result<Json<CaptureGifResponse>, ErrorData> {
    let (duration_ms, interval_ms) = capture_gif_timing(&params)?;
    let max_long_edge = params.max_long_edge.unwrap_or(DEFAULT_MAX_LONG_EDGE);
    if output_path.exists() && !params.overwrite {
        return Err(mcp_error(
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            format!(
                "capture_gif refuses to overwrite existing file without overwrite=true: {:?}",
                params.path
            ),
        ));
    }

    let frames_requested = usize::try_from(duration_ms / interval_ms)
        .unwrap_or(1)
        .max(1);
    let started = Instant::now();
    let mut native_dims: Option<(u32, u32)> = None;
    let mut target_dims: Option<(u32, u32)> = None;
    let mut frames: Vec<RgbaImage> = Vec::with_capacity(frames_requested);
    let mut capture_backend = "graphics_capture_window_bgra".to_owned();
    let (expected_identity, mut last_error) = match identity() {
        Ok(identity) => (Some(identity), None),
        Err(error) => (None, Some(error.to_string())),
    };

    while started.elapsed().as_millis() < u128::from(duration_ms) {
        capture_gif_check_cancelled(&cancel)?;
        let Some(expected_identity) = expected_identity else {
            break;
        };
        let frame_started = Instant::now();
        let mut captured_frame = None;
        match super::m1_tools::capture_with_verified_identity(
            expected_identity,
            false,
            &mut identity,
            || {
                let captured =
                    capture().map_err(|error| mcp_error(error.code(), error.to_string()))?;
                capture_backend = captured.capture_backend.to_owned();
                Ok(captured.bitmap)
            },
            |bitmap| {
                captured_frame = Some(bitmap);
                Ok(serde_json::Value::Null)
            },
        ) {
            Ok(_) => {
                let bitmap = captured_frame.ok_or_else(|| {
                    mcp_error(
                        synapse_core::error_codes::CAPTURE_FRAME_UNAVAILABLE,
                        "GIF capture frame missing",
                    )
                })?;
                if native_dims.is_none() {
                    native_dims = Some((bitmap.width, bitmap.height));
                    target_dims = Some(capture_gif_target_dims(
                        bitmap.width,
                        bitmap.height,
                        max_long_edge,
                    ));
                }
                let rgba = bgra_to_rgba(&bitmap.bytes, bitmap.width, bitmap.height)?;
                let (tw, th) = target_dims.unwrap_or((bitmap.width, bitmap.height));
                let frame = if (tw, th) == (bitmap.width, bitmap.height) {
                    rgba
                } else {
                    image::imageops::resize(&rgba, tw, th, image::imageops::FilterType::Triangle)
                };
                frames.push(frame);
                if frames.len() == 1 {
                    tracing::info!(
                        code = "CAPTURE_GIF_FIRST_FRAME_CAPTURED",
                        window_hwnd,
                        frames_requested,
                        "GIF recording acquired its first verified frame"
                    );
                }
            }
            Err(error) => {
                if error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("code"))
                    .and_then(serde_json::Value::as_str)
                    == Some(synapse_core::error_codes::CAPTURE_TARGET_INVALID)
                {
                    // Reject a live replacement, retain verified frames on
                    // target loss, and retry capture errors for the same target.
                    match identity() {
                        Ok(identity) if identity != expected_identity => return Err(error),
                        Err(_) => {
                            last_error = Some(error.to_string());
                            break;
                        }
                        Ok(_) => {}
                    }
                }
                last_error = Some(error.to_string());
            }
        }
        capture_gif_check_cancelled(&cancel)?;
        // Pace to interval_ms, accounting for capture time.
        let spent = frame_started.elapsed();
        if let Some(remaining) = Duration::from_millis(interval_ms).checked_sub(spent) {
            let pause_started = Instant::now();
            while let Some(left) = remaining.checked_sub(pause_started.elapsed()) {
                capture_gif_check_cancelled(&cancel)?;
                std::thread::sleep(left.min(CANCELLATION_POLL_INTERVAL));
            }
        }
    }

    capture_gif_check_cancelled(&cancel)?;
    if frames.is_empty() {
        return Err(mcp_error(
            synapse_core::error_codes::ACTION_NO_OBSERVED_DELTA,
            format!(
                "capture_gif captured 0 frames of hwnd {window_hwnd:#x} in {duration_ms} ms; last capture error: {}",
                last_error.unwrap_or_else(|| "none".to_owned())
            ),
        ));
    }

    let (tw, th) = target_dims.unwrap_or((1, 1));
    let (nw, nh) = native_dims.unwrap_or((tw, th));
    let frames_captured = frames.len();

    if let Some(parent) = output_path.parent() {
        capture_gif_check_cancelled(&cancel)?;
        std::fs::create_dir_all(parent).map_err(|error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                format!("capture_gif could not create output directory: {error}"),
            )
        })?;
    }
    let output_parent = output_path.parent().ok_or_else(|| {
        mcp_error(
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            format!("capture_gif has no output directory for {:?}", params.path),
        )
    })?;
    capture_gif_check_cancelled(&cancel)?;
    let mut temporary = NamedTempFile::new_in(output_parent).map_err(|error| {
        mcp_error(
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            format!(
                "capture_gif could not create temporary output for {:?}: {error}",
                params.path
            ),
        )
    })?;
    let delay = Delay::from_numer_denom_ms(u32::try_from(interval_ms).unwrap_or(500), 1);
    {
        let mut encoder = GifEncoder::new_with_speed(temporary.as_file_mut(), 10);
        encoder.set_repeat(Repeat::Infinite).map_err(|error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                format!("capture_gif set_repeat failed: {error}"),
            )
        })?;
        for rgba in frames {
            capture_gif_check_cancelled(&cancel)?;
            let frame = Frame::from_parts(rgba, 0, 0, delay);
            encoder.encode_frame(frame).map_err(|error| {
                mcp_error(
                    synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                    format!("capture_gif GIF frame encode failed: {error}"),
                )
            })?;
            capture_gif_check_cancelled(&cancel)?;
        }
    }

    // The cancellation check immediately before synchronous persist is the
    // publication linearization point. Afterwards cancellation is ordered
    // after commit, so this request must never delete the final path.
    capture_gif_check_cancelled(&cancel)?;
    // Each retained frame passed its own identity transaction. Target loss
    // after those frames must not discard a valid partial recording (#102).
    if identity().is_ok_and(|identity| Some(identity) != expected_identity) {
        return Err(mcp_error(
            synapse_core::error_codes::CAPTURE_TARGET_INVALID,
            "GIF target identity changed before artifact publication",
        ));
    }
    capture_gif_check_cancelled(&cancel)?;
    let persisted = if params.overwrite {
        temporary.persist(&output_path)
    } else {
        temporary.persist_noclobber(&output_path)
    };
    persisted.map_err(|error| {
        mcp_error(
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            format!("capture_gif could not publish {:?}: {error}", params.path),
        )
    })?;

    let bytes_written = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(duration_ms);
    tracing::info!(
        code = "CAPTURE_GIF_RECORDED",
        hwnd = window_hwnd,
        frames_captured,
        frames_requested,
        width = tw,
        height = th,
        bytes_written,
        elapsed_ms,
        "readback=capture_gif outcome=encoded"
    );

    Ok(Json(CaptureGifResponse {
        path: output_path.to_string_lossy().into_owned(),
        frames_captured,
        frames_requested,
        width: tw,
        height: th,
        native_width: nw,
        native_height: nh,
        interval_ms,
        duration_ms,
        elapsed_ms,
        bytes_written,
        capture_backend,
        window_hwnd,
    }))
}

fn capture_gif_check_cancelled(cancel: &CancellationToken) -> Result<(), ErrorData> {
    if cancel.is_cancelled() {
        return Err(capture_gif_cancelled_error());
    }
    Ok(())
}

/// #184: the GIF is a screen capture written to a caller-named path, so it
/// follows the shared output-path policy (drive-absolute `.gif`; no UNC,
/// device, alternate-data-stream or reserved-name targets).
pub(super) fn capture_gif_output_path(raw_path: &str) -> Result<PathBuf, ErrorData> {
    CAPTURE_GIF_PATH_POLICY.validate(raw_path)
}

fn capture_gif_cancelled_error() -> ErrorData {
    ErrorData::new(
        ErrorCode(-32800),
        "capture_gif request cancelled",
        Some(json!({
            "code": synapse_core::error_codes::REQUEST_CANCELLED,
            "tool": "capture_gif",
            "operation": "record",
            "source_of_truth": "MCP RequestContext cancellation token",
        })),
    )
}

fn capture_gif_timing(params: &CaptureGifParams) -> Result<(u64, u64), ErrorData> {
    let duration_ms = params.duration_ms.unwrap_or(DEFAULT_DURATION_MS);
    if !(MIN_INTERVAL_MS..=MAX_DURATION_MS).contains(&duration_ms) {
        return Err(capture_gif_bounds_error(
            "duration_ms",
            format!("{MIN_INTERVAL_MS}..={MAX_DURATION_MS}"),
            duration_ms,
            "pass duration_ms between 100 and 60000, or omit it for the default",
        ));
    }

    let interval_ms = params.interval_ms.unwrap_or(DEFAULT_INTERVAL_MS);
    if interval_ms < MIN_INTERVAL_MS {
        return Err(capture_gif_bounds_error(
            "interval_ms",
            format!(">={MIN_INTERVAL_MS}"),
            interval_ms,
            "pass interval_ms >= 100, or omit it for the default",
        ));
    }

    Ok((duration_ms, interval_ms))
}

fn capture_gif_bounds_error(
    field: &'static str,
    accepted_range: String,
    actual_value: u64,
    remediation: &'static str,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!("capture_gif {field} must be {accepted_range}; got {actual_value}"),
        Some(json!({
            "code": synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "tool": "capture_gif",
            "operation": "record",
            "field": field,
            "source_id": field,
            "accepted_range": accepted_range,
            "actual_value": actual_value,
            "source_of_truth": "MCP request parameters",
            "remediation": remediation,
        })),
    )
}

fn capture_gif_target_dims(width: u32, height: u32, max_long_edge: u32) -> (u32, u32) {
    if max_long_edge == 0 || (width <= max_long_edge && height <= max_long_edge) {
        return (width.max(1), height.max(1));
    }
    let long = width.max(height);
    let scale = f64::from(max_long_edge) / f64::from(long);
    let tw = ((f64::from(width) * scale).round() as u32).max(1);
    let th = ((f64::from(height) * scale).round() as u32).max(1);
    (tw, th)
}

fn bgra_to_rgba(bgra: &[u8], width: u32, height: u32) -> Result<RgbaImage, ErrorData> {
    let expected = (width as usize) * (height as usize) * 4;
    if bgra.len() < expected {
        return Err(mcp_error(
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            format!(
                "capture_gif frame byte length {} smaller than {width}x{height}x4={expected}",
                bgra.len()
            ),
        ));
    }
    let mut rgba = vec![0u8; expected];
    for (dst, src) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
        mcp_error(
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            "capture_gif could not build RGBA frame buffer",
        )
    })
}
