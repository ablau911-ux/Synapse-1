//! `capture_gif` MCP tool (#1339).
//!
//! Claude-in-Chrome ships `gif_creator` to record interactions as shareable
//! GIFs. Synapse had `demo_record_*` (UIA JSONL for profile authoring) and
//! terminal asciicast, but no VISUAL screen/browser GIF recorder.
//!
//! `capture_gif` records the bound window (or an explicit HWND) by capturing
//! periodic passive per-window WGC frames over a recording window, then
//! encodes an animated GIF. A browser-tab (CDP) target records that exact tab
//! through the same route `screenshot capture` uses (raw CDP
//! `Page.captureScreenshot`), never the browser window, which can show the
//! human's foreground tab (#286, #1508). It is a single
//! synchronous call — no dangling recording state machine — and reports captured
//! vs requested frame counts so there are never silent frame drops. WGC captures
//! occluded/background windows, so recording does not require foreground.

use std::time::{Duration, Instant};

use image::{
    Delay, Frame, RgbaImage,
    codecs::gif::{GifEncoder, Repeat},
};
use rmcp::{RoleServer, model::ErrorCode, service::RequestContext};
use serde_json::json;

use super::{
    CaptureGifParams, CaptureGifResponse, ErrorData, Json, Parameters, SessionTarget,
    SynapseService, tool, tool_router,
};
use crate::m1::{mcp_error, validate_window_hwnd_shape};

const DEFAULT_DURATION_MS: u64 = 3_000;
pub(super) const MAX_DURATION_MS: u64 = 60_000;
const DEFAULT_INTERVAL_MS: u64 = 500;
pub(super) const MIN_INTERVAL_MS: u64 = 100;
const DEFAULT_MAX_LONG_EDGE: u32 = 800;
const FRAME_TIMEOUT_MS: u64 = 1_500;

#[tool_router(router = capture_gif_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Record the bound window as an animated GIF (Claude gif_creator parity, #1339). Captures periodic passive per-window WGC frames of the session's bound target window — or an explicit window_hwnd — over duration_ms at interval_ms, then encodes an animated GIF to an absolute .gif path. WGC captures occluded/background windows, so recording needs no foreground. With a session browser-tab (CDP) target and no window_hwnd it records that exact tab through the same route as capture_screenshot (raw CDP Page.captureScreenshot), never the browser window, and refuses when that tab has no raw CDP endpoint and no session-owned Chrome bridge owner row. Frames are downscaled aspect-preserving to max_long_edge (default 800). Single synchronous call (no recording state machine); reports frames_captured vs frames_requested so frame drops are never silent, and fails loud on zero frames or encode failure. Use capture_screenshot for a single still."
    )]
    pub async fn capture_gif(
        &self,
        params: Parameters<CaptureGifParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<CaptureGifResponse>, ErrorData> {
        self.require_m3_permissions(
            "capture_gif",
            &crate::m3::permissions::required([crate::m3::permissions::Permission::WriteStorage]),
        )?;
        let params = params.0;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "capture_gif",
            "tool.invocation kind=capture_gif"
        );

        let (duration_ms, interval_ms) = capture_gif_timing(&params)?;
        let max_long_edge = params.max_long_edge.unwrap_or(DEFAULT_MAX_LONG_EDGE);

        let source = self.capture_gif_resolve_source(params.window_hwnd, &request_context)?;

        let output_path = std::path::PathBuf::from(&params.path);
        if !output_path.is_absolute() {
            return Err(mcp_error(
                synapse_core::error_codes::TOOL_PARAMS_INVALID,
                format!("capture_gif path must be absolute: {:?}", params.path),
            ));
        }
        if output_path.exists() && !params.overwrite {
            return Err(mcp_error(
                synapse_core::error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "capture_gif refuses to overwrite existing file without overwrite=true: {:?}",
                    params.path
                ),
            ));
        }

        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            capture_gif_blocking(
                params,
                source,
                &runtime,
                output_path,
                duration_ms,
                interval_ms,
                max_long_edge,
            )
        })
        .await
        .map_err(|error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                format!("capture_gif blocking worker failed: {error}"),
            )
        })?
    }

    /// An explicit window_hwnd or a window target records that window; a
    /// browser-tab (CDP) target records the exact tab through the same route
    /// `screenshot capture` uses, never the browser window (#286).
    fn capture_gif_resolve_source(
        &self,
        explicit: Option<i64>,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<GifSource, ErrorData> {
        if let Some(hwnd) = explicit {
            return validate_window_hwnd_shape("capture_gif", hwnd).map(GifSource::Window);
        }
        let session_id = super::context::mcp_session_id_from_request_context(request_context)?;
        match self.session_target(session_id.as_deref())? {
            Some(SessionTarget::Window { hwnd }) => {
                validate_window_hwnd_shape("capture_gif", hwnd).map(GifSource::Window)
            }
            Some(SessionTarget::Cdp {
                window_hwnd,
                cdp_target_id,
            }) => {
                let window_hwnd = validate_window_hwnd_shape("capture_gif", window_hwnd)?;
                let session_id = session_id.as_deref().ok_or_else(|| {
                    mcp_error(
                        synapse_core::error_codes::HTTP_SESSION_INVALID,
                        "capture_gif requires an MCP session id for a CDP session target",
                    )
                })?;
                self.resolve_cdp_tab_capture("capture_gif", session_id, window_hwnd, &cdp_target_id)
                    .map(GifSource::Tab)
            }
            None => Err(mcp_error(
                synapse_core::error_codes::TARGET_NOT_SET,
                "capture_gif requires a window_hwnd or a bound session target (set_target)",
            )),
        }
    }
}

/// One captured frame with its backend name, or the reason it was dropped.
type GifFrame = Result<(synapse_capture::CapturedBgraBitmap, &'static str), String>;

/// What one GIF records: a window through passive per-window WGC, or one
/// browser tab through the exact-tab capture route (#286).
enum GifSource {
    Window(i64),
    Tab(super::m1_tools::CdpTabCapture),
}

impl GifSource {
    fn window_hwnd(&self) -> i64 {
        match self {
            Self::Window(hwnd) => *hwnd,
            Self::Tab(tab) => tab.window_hwnd(),
        }
    }

    /// Captures one frame. `Ok(Err(..))` is a dropped frame that the loop
    /// counts and keeps going; `Err(..)` ends the recording with that error.
    fn grab_frame(
        &self,
        runtime: &tokio::runtime::Handle,
        frames_so_far: usize,
    ) -> Result<GifFrame, ErrorData> {
        match self {
            Self::Window(hwnd) => {
                match synapse_capture::window_full_frame_to_bgra_bitmap(*hwnd, FRAME_TIMEOUT_MS) {
                    Ok(captured) => Ok(Ok((captured.bitmap, captured.capture_backend))),
                    // An invalid target (for example a minimized window) stays
                    // invalid for the whole recording: fail now with its own code.
                    Err(error @ synapse_capture::CaptureError::TargetInvalid { .. }) => {
                        Err(mcp_error(
                            error.code(),
                            format!("capture_gif cannot record hwnd {hwnd:#x}: {error}"),
                        ))
                    }
                    Err(error) => Ok(Err(error.to_string())),
                }
            }
            Self::Tab(tab) => match runtime.block_on(tab.grab_frame(None)) {
                Ok(frame) => Ok(Ok(frame)),
                // No frame yet: the tab route itself is refusing or broken,
                // so report its own error instead of recording nothing.
                Err(error) if frames_so_far == 0 => Err(error),
                Err(error) => Ok(Err(error.message.to_string())),
            },
        }
    }
}

// Capture, pixel conversion, resizing, encoding and file I/O all block.
// Keep the whole recording on the blocking pool, including interval pacing;
// a tab frame's async CDP read is driven from here with `runtime.block_on`.
fn capture_gif_blocking(
    params: CaptureGifParams,
    source: GifSource,
    runtime: &tokio::runtime::Handle,
    output_path: std::path::PathBuf,
    duration_ms: u64,
    interval_ms: u64,
    max_long_edge: u32,
) -> Result<Json<CaptureGifResponse>, ErrorData> {
    let window_hwnd = source.window_hwnd();
    let frames_requested = usize::try_from(duration_ms / interval_ms)
        .unwrap_or(1)
        .max(1);
    let started = Instant::now();
    let mut native_dims: Option<(u32, u32)> = None;
    let mut target_dims: Option<(u32, u32)> = None;
    let mut frames: Vec<RgbaImage> = Vec::with_capacity(frames_requested);
    let mut capture_backend = "graphics_capture_window_bgra".to_owned();
    let mut last_error: Option<String> = None;

    while started.elapsed().as_millis() < u128::from(duration_ms) {
        let frame_started = Instant::now();
        match source.grab_frame(runtime, frames.len())? {
            Ok((bitmap, backend)) => {
                capture_backend = backend.to_owned();
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
            }
            Err(error) => {
                last_error = Some(error);
            }
        }
        // Pace to interval_ms, accounting for capture time, but never sleep
        // past duration_ms: an interval longer than the duration must not
        // stretch the call beyond the requested recording length.
        let spent = frame_started.elapsed();
        let until_deadline = Duration::from_millis(duration_ms).saturating_sub(started.elapsed());
        if let Some(remaining) = Duration::from_millis(interval_ms).checked_sub(spent) {
            std::thread::sleep(remaining.min(until_deadline));
        }
    }

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
        std::fs::create_dir_all(parent).map_err(|error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                format!("capture_gif could not create output directory: {error}"),
            )
        })?;
    }
    let file = std::fs::File::create(&output_path).map_err(|error| {
        mcp_error(
            synapse_core::error_codes::TOOL_INTERNAL_ERROR,
            format!("capture_gif could not create {:?}: {error}", params.path),
        )
    })?;
    let delay = Delay::from_numer_denom_ms(u32::try_from(interval_ms).unwrap_or(500), 1);
    {
        let mut encoder = GifEncoder::new_with_speed(file, 10);
        encoder.set_repeat(Repeat::Infinite).map_err(|error| {
            mcp_error(
                synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                format!("capture_gif set_repeat failed: {error}"),
            )
        })?;
        for rgba in frames {
            let frame = Frame::from_parts(rgba, 0, 0, delay);
            encoder.encode_frame(frame).map_err(|error| {
                mcp_error(
                    synapse_core::error_codes::TOOL_INTERNAL_ERROR,
                    format!("capture_gif GIF frame encode failed: {error}"),
                )
            })?;
        }
    }

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
