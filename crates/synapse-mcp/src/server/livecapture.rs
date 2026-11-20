//! Inline, cursor-based visual sampling from the continuous capture owner.
//! MCP delivers bounded image batches per call; it does not stream model inference.

use super::{
    ErrorData, Parameters, SynapseService,
    livecapture_ownership::{ensure_capture_mutation_allowed, failed_start_should_stop},
    m1_tools::{downscale_captured_bitmap, sha256_hex},
    mcp_error, tool, tool_router,
};
use crate::m1::{
    CaptureTargetParam, SetCaptureTargetParams, set_capture_target_in_state,
    stop_capture_target_in_state,
};
use base64::Engine as _;
use image::{DynamicImage, RgbaImage, codecs::jpeg::JpegEncoder};
use rmcp::{
    RoleServer,
    model::{CallToolResult, Content},
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, de};
use serde_json::{Value, json};
use std::{
    fmt,
    time::{Duration, Instant},
};
use synapse_core::{CaptureFrameCursor, Rect, error_codes};

const SOURCE: &str = "M1 continuous capture owner + inline JPEG bytes + frame cursors";
const DEFAULT_MAX_LONG_EDGE: u32 = 1280;
const DEFAULT_INTERVAL_MS: u64 = 200;
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 2000;
const START_WAIT_TIMEOUT_MS: u64 = 5000;

/// Reduces a client-supplied cursor value to the strict `CaptureFrameCursor`
/// fields. Real clients echo back the richer cursor-bearing object this tool
/// itself emits (`dirty_region`, `frame_reused`, frame metadata, ...), so the
/// extra keys are dropped here instead of rejecting the whole call; a missing
/// or wrongly-typed essential field still fails loudly.
fn normalize_echoed_cursor(value: Value) -> Result<CaptureFrameCursor, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictCursor {
        capture_id: String,
        generation: u64,
        frame_seq: u64,
    }
    let Value::Object(fields) = value else {
        return Err("cursor payload must be a JSON object".to_owned());
    };
    let essential = ["capture_id", "generation", "frame_seq"];
    let filtered: serde_json::Map<String, Value> = fields
        .into_iter()
        .filter(|(key, _)| essential.contains(&key.as_str()))
        .collect();
    for key in essential {
        if !filtered.contains_key(key) {
            return Err(format!("missing field `{key}`"));
        }
    }
    serde_json::from_value::<StrictCursor>(Value::Object(filtered))
        .map(|cursor| CaptureFrameCursor {
            capture_id: cursor.capture_id,
            generation: cursor.generation,
            frame_seq: cursor.frame_seq,
        })
        .map_err(|error| error.to_string())
}

fn deserialize_capture_frame_cursor<'de, D>(deserializer: D) -> Result<CaptureFrameCursor, D::Error>
where
    D: Deserializer<'de>,
{
    struct CursorVisitor;

    impl<'de> de::Visitor<'de> for CursorVisitor {
        type Value = CaptureFrameCursor;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a capture cursor object or a JSON string encoding one")
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            let value = Value::deserialize(de::value::MapAccessDeserializer::new(map))?;
            normalize_echoed_cursor(value).map_err(de::Error::custom)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            let parsed: Value = serde_json::from_str(value).map_err(|error| {
                E::custom(format!(
                    "after_frame JSON string must encode a valid capture cursor object: {error}"
                ))
            })?;
            normalize_echoed_cursor(parsed).map_err(|error| {
                E::custom(format!(
                    "after_frame JSON string must encode a valid capture cursor object: {error}"
                ))
            })
        }
    }

    deserializer.deserialize_any(CursorVisitor)
}

fn capture_frame_cursor_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "anyOf": [
            {
                "type": "object",
                "required": ["capture_id", "generation", "frame_seq"],
                "properties": {
                    "capture_id": { "type": "string" },
                    "generation": { "type": "integer", "minimum": 0, "maximum": u64::MAX },
                    "frame_seq": { "type": "integer", "minimum": 0, "maximum": u64::MAX }
                }
            },
            { "type": "string" }
        ]
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LiveCaptureParams {
    /// Start continuous capture and return the first frame directly as an image.
    Start {
        window_hwnd: i64,
        #[serde(default)]
        max_long_edge: Option<u32>,
    },
    /// Return up to six fresh sampled frames, each strictly after the cursor.
    Next {
        window_hwnd: i64,
        #[serde(deserialize_with = "deserialize_capture_frame_cursor")]
        #[schemars(schema_with = "capture_frame_cursor_schema")]
        after_frame: CaptureFrameCursor,
        #[serde(default)]
        frame_count: Option<u32>,
        #[serde(default)]
        interval_ms: Option<u64>,
        #[serde(default)]
        wait_timeout_ms: Option<u64>,
        #[serde(default)]
        max_long_edge: Option<u32>,
    },
    Status,
    Stop,
}

#[derive(Debug)]
struct LiveFrameError {
    code: &'static str,
    reason: &'static str,
    detail: String,
}

impl LiveFrameError {
    fn new(code: &'static str, reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            reason,
            detail: detail.into(),
        }
    }

    fn unavailable(reason: &'static str, detail: impl Into<String>) -> Self {
        Self::new(error_codes::CAPTURE_FRAME_UNAVAILABLE, reason, detail)
    }

    fn into_error_data(self) -> ErrorData {
        mcp_error(self.code, format!("{}: {}", self.reason, self.detail))
    }
}

struct LiveFrame {
    bitmap: synapse_capture::CapturedBgraBitmap,
    capture_id: String,
    generation: u64,
    frame_seq: u64,
    native_width: u32,
    native_height: u32,
    dirty_region: Option<Rect>,
    captured_at: Instant,
    frame_reused: bool,
}

struct EncodedFrame {
    jpeg_base64: String,
    image_sha256: String,
    width: u32,
    height: u32,
    scale: f64,
}

fn invalid(detail: impl Into<String>) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, detail.into())
}

fn livecapture_config(
    current: &synapse_capture::CaptureConfig,
    hwnd: i64,
) -> synapse_capture::CaptureConfig {
    synapse_capture::CaptureConfig {
        target: synapse_capture::CaptureTarget::Window { hwnd },
        min_update_interval_ms: 100,
        cursor_visible: true,
        dirty_region_only: false,
        ..current.clone()
    }
}

fn image_size(value: Option<u32>) -> Result<u32, ErrorData> {
    let size = value.unwrap_or(DEFAULT_MAX_LONG_EDGE);
    if !(128..=2048).contains(&size) {
        return Err(invalid("max_long_edge must be in 128..=2048"));
    }
    Ok(size)
}

fn text_result(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![Content::text(value.to_string())]);
    result.structured_content = Some(value);
    result
}

fn validate_batch_params(
    frame_count: Option<u32>,
    interval_ms: Option<u64>,
    wait_timeout_ms: Option<u64>,
) -> Result<(u32, u64, u64), ErrorData> {
    let count = frame_count.unwrap_or(3);
    let interval = interval_ms.unwrap_or(DEFAULT_INTERVAL_MS);
    let timeout = wait_timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    if !(1..=6).contains(&count)
        || !(100..=1000).contains(&interval)
        || !(1..=5000).contains(&timeout)
    {
        return Err(invalid(
            "frame_count must be 1..=6, interval_ms 100..=1000, wait_timeout_ms 1..=5000",
        ));
    }
    Ok((count, interval, timeout))
}

fn full_frame_region(width: u32, height: u32) -> Result<Rect, LiveFrameError> {
    Ok(Rect {
        x: 0,
        y: 0,
        w: i32::try_from(width).map_err(|_| {
            LiveFrameError::new(
                error_codes::TOOL_INTERNAL_ERROR,
                "frame_dimensions_invalid",
                format!("captured frame width {width} exceeds i32::MAX"),
            )
        })?,
        h: i32::try_from(height).map_err(|_| {
            LiveFrameError::new(
                error_codes::TOOL_INTERNAL_ERROR,
                "frame_dimensions_invalid",
                format!("captured frame height {height} exceeds i32::MAX"),
            )
        })?,
    })
}

#[cfg(windows)]
fn frame_to_bgra_bitmap(
    frame: &synapse_capture::CapturedFrame,
) -> Result<synapse_capture::CapturedBgraBitmap, LiveFrameError> {
    let region = full_frame_region(frame.width, frame.height)?;
    synapse_capture::captured_frame_region_to_bgra_bitmap(frame, region)
        .map_err(|error| LiveFrameError::new(error.code(), "frame_copy_failed", error.to_string()))
}

#[cfg(not(windows))]
fn frame_to_bgra_bitmap(
    _frame: &synapse_capture::CapturedFrame,
) -> Result<synapse_capture::CapturedBgraBitmap, LiveFrameError> {
    Err(LiveFrameError::new(
        error_codes::CAPTURE_GRAPHICS_API_UNSUPPORTED,
        "windows_capture_required",
        "livecapture requires Windows Graphics Capture frame readback",
    ))
}

fn encode_jpeg(
    bitmap: synapse_capture::CapturedBgraBitmap,
    max_long_edge: u32,
) -> Result<EncodedFrame, LiveFrameError> {
    let (bitmap, scale) =
        downscale_captured_bitmap(bitmap, None, Some(max_long_edge)).map_err(|error| {
            LiveFrameError::new(
                error_codes::TOOL_INTERNAL_ERROR,
                "frame_downscale_failed",
                format!("{error:?}"),
            )
        })?;
    let mut rgba = bitmap.bytes;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image = RgbaImage::from_raw(bitmap.width, bitmap.height, rgba).ok_or_else(|| {
        LiveFrameError::new(
            error_codes::TOOL_INTERNAL_ERROR,
            "image_buffer_invalid",
            format!(
                "could not build image buffer from {}x{} captured bitmap",
                bitmap.width, bitmap.height
            ),
        )
    })?;
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 85)
        .encode_image(&DynamicImage::ImageRgba8(image))
        .map_err(|error| {
            LiveFrameError::new(
                error_codes::TOOL_INTERNAL_ERROR,
                "jpeg_encode_failed",
                error.to_string(),
            )
        })?;
    Ok(EncodedFrame {
        jpeg_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        image_sha256: sha256_hex(&bytes),
        width: bitmap.width,
        height: bitmap.height,
        scale,
    })
}

#[tool_router(router = livecapture_tool_router, vis = "pub(crate)")]
impl SynapseService {
    #[tool(
        input_schema = super::schema_sanitize::tagged_object_input_schema::<LiveCaptureParams>(),
        description = "Live visual perception from a native window. start enables continuous WGC capture and returns an inline JPEG image; repeated start on the same window with an equivalent running capture preserves its capture_id, generation and existing cursors. If no new frame is queued, start returns the last copied bitmap marked frame_reused=true with its original frame_seq and sample_age_ms. next returns ordered fresh inline JPEG images with timing, sequence gaps and a resumable capture cursor; after_frame accepts the returned next_cursor object or a JSON string encoding that exact object for clients that stringify nested tool arguments. status reads the capture runtime; stop ends capture. The session that started a capture owns it: another live session may join an equivalent start and read frames, but stop or a retargeting start by a non-owner is refused with CAPTURE_OWNED_BY_OTHER_SESSION naming the owner session (status reports the owner); a terminated owner releases it. Images are delivered directly in MCP content, with no screenshot/file round trip. Each next call is bounded (1..6 frames, 1..5000 ms wait per frame) and never replays an old frame as new. Concurrent callers share one non-destructive frame feed (the newest 4 frames are retained): a caller never removes a frame another cursor still needs, and a cursor older than the retained window reports the missed frames in skipped_sequence_numbers. If the capture worker ends, next fails fast with capture_stopped instead of waiting out the timeout. The daemon has one shared capture owner; a restart, stop, or target change invalidates old cursors. Native windows only; minimized/closed targets fail. No automatic model attention, file capture, audio recording, or background recording."
    )]
    pub async fn livecapture(
        &self,
        params: Parameters<LiveCaptureParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match params.0 {
            LiveCaptureParams::Start {
                window_hwnd,
                max_long_edge,
            } => {
                let size = image_size(max_long_edge)?;
                let caller = super::context::mcp_session_id_from_request_context(&context)?;
                let open_sessions = self.open_session_ids()?;
                let started_capture = {
                    // Check and start under one lock so simultaneous retries
                    // cannot both replace the same stream and invalidate cursors.
                    let mut state = self.m1_state()?;
                    let config = livecapture_config(&state.capture_config, window_hwnd);
                    synapse_capture::resolve_capture_target(&config)
                        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
                    let reuse = state
                        .capture_controller
                        .active()
                        .is_some_and(|handle| handle.is_running() && handle.config() == &config);
                    if !reuse {
                        // Retargeting would end another live session's capture (#112).
                        ensure_capture_mutation_allowed(
                            &state,
                            "start",
                            caller.as_deref(),
                            &open_sessions,
                        )?;
                        set_capture_target_in_state(
                            &mut state,
                            SetCaptureTargetParams {
                                target: CaptureTargetParam::Window { window_hwnd },
                                min_update_interval_ms: Some(config.min_update_interval_ms),
                                cursor_visible: Some(config.cursor_visible),
                                dirty_region_only: Some(config.dirty_region_only),
                            },
                        )?;
                    }
                    if reuse {
                        // Adopt the running capture so the start that created it
                        // does not stop it from under this caller on failure (#110).
                        // The adopter still remembers it: if the worker later dies,
                        // its failed start must clean the dead capture up (#122).
                        state.capture_controller.active().map(|handle| {
                            handle.feed().join();
                            (handle.capture_id().to_owned(), false)
                        })
                    } else {
                        let created = state
                            .capture_controller
                            .active()
                            .map(|handle| (handle.capture_id().to_owned(), true));
                        if let Some((capture_id, _)) = created.as_ref() {
                            state.capture_owner.claim(capture_id, caller.as_deref());
                        }
                        created
                    }
                };
                let result = self
                    .livecapture_batch(
                        window_hwnd,
                        None,
                        1,
                        DEFAULT_INTERVAL_MS,
                        START_WAIT_TIMEOUT_MS,
                        size,
                        &context,
                    )
                    .await;
                if result.is_err()
                    && let Some((started_capture_id, created_here)) = started_capture
                {
                    let mut state = self.m1_state()?;
                    // A healthy capture another start adopted must survive (#110),
                    // but a worker that already ended is cleaned up whoever
                    // adopted it (#122). A worker can fail after start accepted
                    // its HWND but before it emits a frame; stopping surfaces that
                    // terminal error instead of a timeout.
                    let should_stop = state.capture_controller.active().is_some_and(|handle| {
                        handle.capture_id() == started_capture_id
                            && failed_start_should_stop(
                                created_here,
                                handle.feed().has_joiners(),
                                handle.is_running(),
                            )
                    });
                    if should_stop {
                        stop_capture_target_in_state(&mut state)?;
                    }
                }
                result
            }
            LiveCaptureParams::Next {
                window_hwnd,
                after_frame,
                frame_count,
                interval_ms,
                wait_timeout_ms,
                max_long_edge,
            } => {
                let (count, interval, timeout) =
                    validate_batch_params(frame_count, interval_ms, wait_timeout_ms)?;
                self.livecapture_batch(
                    window_hwnd,
                    Some(after_frame),
                    count,
                    interval,
                    timeout,
                    image_size(max_long_edge)?,
                    &context,
                )
                .await
            }
            LiveCaptureParams::Status => Ok(text_result(json!({
                "operation": "status",
                "source_of_truth": SOURCE,
                "runtime": self.dashboard_capture_runtime_readback()?,
                "owner": self.livecapture_owner_readback()?,
            }))),
            LiveCaptureParams::Stop => {
                let caller = super::context::mcp_session_id_from_request_context(&context)?;
                let open_sessions = self.open_session_ids()?;
                let (stopped, runtime) = {
                    let mut state = self.m1_state()?;
                    ensure_capture_mutation_allowed(
                        &state,
                        "stop",
                        caller.as_deref(),
                        &open_sessions,
                    )?;
                    stop_capture_target_in_state(&mut state)?
                };
                Ok(text_result(json!({
                    "operation": "stop",
                    "source_of_truth": SOURCE,
                    "stopped": stopped,
                    "runtime": runtime,
                })))
            }
        }
    }

    async fn livecapture_batch(
        &self,
        hwnd: i64,
        mut cursor: Option<CaptureFrameCursor>,
        count: u32,
        interval: u64,
        timeout: u64,
        size: u32,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let mut content = Vec::new();
        let mut frames = Vec::new();
        let mut timed_out = false;

        for index in 0..count {
            if context.ct.is_cancelled() {
                return Err(invalid("livecapture request cancelled"));
            }
            if index > 0 {
                tokio::select! {
                    () = context.ct.cancelled() => return Err(invalid("livecapture request cancelled")),
                    () = tokio::time::sleep(Duration::from_millis(interval)) => {}
                }
            }

            let previous_seq = cursor.as_ref().map(|current| current.frame_seq);
            let frame = match self
                .next_live_frame(hwnd, cursor.as_ref(), Duration::from_millis(timeout))
                .await
            {
                Ok(frame) => frame,
                Err(error) if error.reason == "after_frame_timeout" => {
                    if frames.is_empty() {
                        return Err(error.into_error_data());
                    }
                    timed_out = true;
                    break;
                }
                Err(error) => return Err(error.into_error_data()),
            };

            let next_cursor = CaptureFrameCursor {
                capture_id: frame.capture_id.clone(),
                generation: frame.generation,
                frame_seq: frame.frame_seq,
            };
            let frame_dirty_region = frame.dirty_region;
            let frame_sample_age_ms = frame.captured_at.elapsed().as_millis();
            let frame_reused = frame.frame_reused;
            let frame_native_width = frame.native_width;
            let frame_native_height = frame.native_height;
            let encoded = tokio::task::spawn_blocking(move || encode_jpeg(frame.bitmap, size))
                .await
                .map_err(|error| invalid(error.to_string()))?
                .map_err(LiveFrameError::into_error_data)?;

            let metadata = json!({
                "index": index,
                "frame": {
                    "capture_id": next_cursor.capture_id.clone(),
                    "generation": next_cursor.generation,
                    "frame_seq": next_cursor.frame_seq,
                    "dirty_region": frame_dirty_region,
                    "sample_age_ms": frame_sample_age_ms,
                    "frame_reused": frame_reused,
                },
                "received_at_utc": chrono::Utc::now().to_rfc3339(),
                "elapsed_ms": started.elapsed().as_millis(),
                "native_width": frame_native_width,
                "native_height": frame_native_height,
                "width": encoded.width,
                "height": encoded.height,
                "scale": encoded.scale,
                "image_sha256": encoded.image_sha256,
                "skipped_sequence_numbers": previous_seq.map(|seq| next_cursor.frame_seq.saturating_sub(seq).saturating_sub(1)),
            });
            cursor = Some(next_cursor);
            content.push(Content::text(metadata.to_string()));
            content.push(Content::image(encoded.jpeg_base64, "image/jpeg"));
            frames.push(metadata);
        }

        if context.ct.is_cancelled() {
            return Err(invalid("livecapture request cancelled"));
        }
        let metadata = json!({
            "source_of_truth": SOURCE,
            "window_hwnd": hwnd,
            "frames": frames,
            "frame_count": frames.len(),
            "requested_frames": count,
            "timed_out": timed_out,
            "next_cursor": cursor,
            "elapsed_ms": started.elapsed().as_millis(),
            "delivery": "sampled_inline_images",
            "runtime": self.dashboard_capture_runtime_readback()?,
        });
        content.insert(0, Content::text(metadata.to_string()));
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(metadata);
        Ok(result)
    }

    async fn next_live_frame(
        &self,
        hwnd: i64,
        after: Option<&CaptureFrameCursor>,
        timeout: Duration,
    ) -> Result<LiveFrame, LiveFrameError> {
        let (feed, generation, capture_id, cached_frame) = {
            let state = self.m1_state().map_err(|error| {
                LiveFrameError::new(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "m1_state_unavailable",
                    format!("{error:?}"),
                )
            })?;
            let Some(handle) = state.capture_controller.active() else {
                return Err(LiveFrameError::unavailable(
                    "capture_inactive",
                    "livecapture requires an active continuous capture owner; call start first",
                ));
            };
            match &handle.target().target {
                synapse_capture::CaptureTarget::Window { hwnd: active_hwnd }
                    if *active_hwnd == hwnd => {}
                other => {
                    return Err(LiveFrameError::unavailable(
                        "target_mismatch",
                        format!(
                            "active capture target {other:?} does not match requested hwnd {hwnd:#x}"
                        ),
                    ));
                }
            }
            (
                handle.feed(),
                state.capture_controller.generation(),
                handle.capture_id().to_owned(),
                if after.is_none() {
                    handle.cached_frame().cloned()
                } else {
                    None
                },
            )
        };

        let min_seq = match after {
            Some(cursor) if cursor.capture_id != capture_id || cursor.generation != generation => {
                return Err(LiveFrameError::unavailable(
                    "cursor_stale",
                    "after_frame cursor does not belong to the active capture owner",
                ));
            }
            Some(cursor) => Some(cursor.frame_seq),
            None => None,
        };

        let frame = tokio::task::spawn_blocking(move || {
            // Every reader goes through the shared feed, so concurrent callers
            // never take frames from each other (#111).
            let fresh = |frame: &synapse_capture::CapturedFrame| {
                Ok::<_, LiveFrameError>(LiveFrame {
                    bitmap: frame_to_bgra_bitmap(frame)?,
                    capture_id: capture_id.clone(),
                    generation,
                    frame_seq: frame.frame_seq,
                    native_width: frame.width,
                    native_height: frame.height,
                    dirty_region: frame.dirty_region,
                    captured_at: frame.captured_at,
                    frame_reused: false,
                })
            };
            if min_seq.is_none() {
                // A static WGC window may emit no new frames. Only start may
                // return the last copied CPU bitmap, with its real age/sequence.
                // next never reads this cache and still waits strictly after its cursor.
                let floor = cached_frame.as_ref().map(|cached| cached.frame_seq);
                if let Some(frame) = feed.try_latest_after(floor) {
                    return fresh(&frame);
                }
                if let Some(cached) = cached_frame {
                    return Ok(LiveFrame {
                        native_width: cached.bitmap.width,
                        native_height: cached.bitmap.height,
                        bitmap: cached.bitmap,
                        capture_id: capture_id.clone(),
                        generation,
                        frame_seq: cached.frame_seq,
                        dirty_region: cached.dirty_region,
                        captured_at: cached.captured_at,
                        frame_reused: true,
                    });
                }
            }
            match feed.wait_after(min_seq, timeout) {
                Ok(frame) => fresh(&frame),
                Err(synapse_capture::FeedWaitError::Timeout) => Err(LiveFrameError::unavailable(
                    "after_frame_timeout",
                    format!(
                        "timed out after {} ms waiting for a frame after {:?}",
                        timeout.as_millis(),
                        min_seq
                    ),
                )),
                Err(synapse_capture::FeedWaitError::Disconnected) => {
                    Err(LiveFrameError::unavailable(
                        "capture_stopped",
                        "the capture worker ended before a frame after the cursor arrived",
                    ))
                }
            }
        })
        .await
        .map_err(|error| {
            LiveFrameError::new(
                error_codes::TOOL_INTERNAL_ERROR,
                "wait_task_failed",
                error.to_string(),
            )
        })??;
        if !frame.frame_reused {
            let mut state = self.m1_state().map_err(|error| {
                LiveFrameError::new(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "m1_state_unavailable",
                    format!("{error:?}"),
                )
            })?;
            if !state.capture_controller.remember_frame(
                &frame.capture_id,
                synapse_capture::CachedCaptureFrame {
                    bitmap: frame.bitmap.clone(),
                    frame_seq: frame.frame_seq,
                    dirty_region: frame.dirty_region,
                    captured_at: frame.captured_at,
                },
            ) {
                return Err(LiveFrameError::unavailable(
                    "cursor_stale",
                    "capture changed during frame readback",
                ));
            }
        }
        Ok(frame)
    }
}

#[cfg(test)]
#[path = "tests/livecapture_cursor_109.rs"]
mod livecapture_cursor_109_tests;

#[cfg(test)]
#[path = "tests/livecapture_failed_start_122.rs"]
mod livecapture_failed_start_122_tests;

#[cfg(test)]
#[path = "tests/livecapture_cursor_schema_dispatch_109.rs"]
mod livecapture_cursor_schema_dispatch_109_tests;
